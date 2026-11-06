use evento::ProjectionAggregate;
use evento::{Executor, WriteError};
use timada_core::claim::claim_is_fresh;

use crate::aggregator::{Claimed, Released};
use crate::error::DiscountError;
use crate::value_object::discount_code_id;

use super::{Command, DiscountCodeState};

/// How many times a claim or a release reads the entry again after losing
/// a race for it.
const CLAIM_ATTEMPTS: usize = 3;

impl<E: Executor> Command<E> {
    /// Points a normalized code at `discount_id`. The entry is created, or
    /// taken over when it is free, already the discount's, or held by a
    /// discount that let go of it. Fails when another discount holds it, or
    /// wins the race for it.
    pub(crate) async fn claim_code(
        &self,
        code: &str,
        discount_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let claimed = Claimed {
            discount_id: discount_id.to_owned(),
        };
        let created = evento::append(discount_code_id(code))
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&claimed)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match created {
            Ok(_) => return Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(existing) = self.load_code_index_any(code).await? else {
                return Err(DiscountError::CodeTaken);
            };
            let free = existing.released
                || existing.discount_id == discount_id
                || self.code_let_go(&existing).await?;
            if !free {
                return Err(DiscountError::CodeTaken);
            }
            let taken = existing
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&claimed)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match taken {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(DiscountError::CodeTaken)
    }

    /// Whether the discount an entry names has let go of the code:
    /// archived, or never written once the claim is no longer fresh. A
    /// discount keeps its code for life.
    async fn code_let_go(&self, index: &DiscountCodeState) -> anyhow::Result<bool> {
        Ok(match self.load_discount(&index.discount_id).await? {
            Some(discount) => discount.is_archived(),
            None => !claim_is_fresh(index.claimed_at, timada_core::unix_now()),
        })
    }

    /// Frees the entry of a code owned by `discount_id` once the discount
    /// holds it no more: archived, or never written. A missing or already
    /// freed entry is fine; one owned by another discount is left alone.
    /// Told twice, nothing more happens.
    pub(crate) async fn release_code(
        &self,
        code: &str,
        discount_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let held = self
            .load_discount(discount_id)
            .await?
            .is_some_and(|discount| !discount.is_archived());
        if held {
            return Ok(());
        }
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(index) = self.load_code_index(code).await? else {
                return Ok(());
            };
            if index.discount_id != discount_id {
                return Ok(());
            }
            let released = index
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Released)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match released {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// Frees the code of an archived discount, for the release
    /// subscription ([`crate::release_subscription`]).
    pub(crate) async fn release_given_up(
        &self,
        discount_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        let Some(discount) = self.load_discount(discount_id).await? else {
            return Ok(());
        };
        self.release_code(&discount.code, discount_id, routing_key)
            .await
    }
}
