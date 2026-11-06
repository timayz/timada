use evento::ProjectionAggregate;
use evento::{Executor, WriteError};
use timada_core::claim::claim_is_fresh;

use crate::aggregator::{Claimed, Released};
use crate::error::TenancyError;
use crate::value_object::tenant_slug_id;

use super::{Command, TenantSlugState};

/// How many times a claim or a release reads the entry again after losing
/// a race for it.
const CLAIM_ATTEMPTS: usize = 3;

impl<E: Executor> Command<E> {
    /// Points a normalized slug at `tenant_id`. The entry is created, or
    /// taken over when it is free, already the tenant's, or dangling: its
    /// tenant never got written and the claim is no longer fresh (a node
    /// died between the claim and the tenant). Fails when another tenant
    /// holds it, or wins the race for it.
    pub(crate) async fn claim_slug(
        &self,
        slug: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let claimed = Claimed {
            tenant_id: tenant_id.to_owned(),
        };
        let created = evento::append(tenant_slug_id(slug))
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&claimed)
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => return Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(existing) = self.load_slug_index_any(slug).await? else {
                return Err(TenancyError::SlugTaken);
            };
            let free = existing.released
                || existing.tenant_id == tenant_id
                || self.slug_dangling(&existing).await?;
            if !free {
                return Err(TenancyError::SlugTaken);
            }
            let taken = existing
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&claimed)
                .commit(&self.0)
                .await;
            match taken {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(TenancyError::SlugTaken)
    }

    /// Whether an entry's tenant never got written and the claim is no
    /// longer fresh. A tenant keeps its slug for life.
    async fn slug_dangling(&self, index: &TenantSlugState) -> anyhow::Result<bool> {
        Ok(!claim_is_fresh(index.claimed_at, timada_core::unix_now())
            && self.load_tenant(&index.tenant_id).await?.is_none())
    }

    /// Gives back the slug a tenant that could not be created claimed. A
    /// missing or already freed entry is fine; one owned by another tenant,
    /// or by a tenant written after all, is left alone.
    pub(crate) async fn release_slug(
        &self,
        slug: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        if self.load_tenant(tenant_id).await?.is_some() {
            return Ok(());
        }
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(index) = self.load_slug_index(slug).await? else {
                return Ok(());
            };
            if index.tenant_id != tenant_id {
                return Ok(());
            }
            let released = index
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Released)
                .commit(&self.0)
                .await;
            match released {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}
