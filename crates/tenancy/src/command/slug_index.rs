use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Claimed, Released};
use crate::error::TenancyError;
use crate::value_object::tenant_slug_id;

use super::Command;

impl<E: Executor> Command<E> {
    /// Points a normalized slug at `tenant_id`, creating or re-claiming the
    /// index entry. Fails when it already points at another tenant.
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
            .routing_key_opt(routing_key)
            .event(&claimed)
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_slug_index_any(slug).await? else {
                    return Err(TenancyError::SlugTaken);
                };
                if existing.released {
                    existing.write()?.event(&claimed).commit(&self.0).await?;
                    Ok(())
                } else if existing.tenant_id == tenant_id {
                    Ok(())
                } else {
                    Err(TenancyError::SlugTaken)
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Frees the index entry of a slug owned by `tenant_id`. A missing or
    /// already freed entry is fine; one owned by another tenant is left alone.
    pub(crate) async fn release_slug(
        &self,
        slug: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        if let Some(index) = self.load_slug_index(slug).await?
            && index.tenant_id == tenant_id
        {
            index
                .write()?
                .routing_key_opt(routing_key)
                .event(&Released)
                .commit(&self.0)
                .await?;
        }
        Ok(())
    }
}
