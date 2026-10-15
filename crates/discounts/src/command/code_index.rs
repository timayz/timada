use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Claimed, Released};
use crate::error::DiscountError;
use crate::value_object::discount_code_id;

use super::Command;

impl<E: Executor> Command<E> {
    /// Points a normalized code at `discount_id`, creating or re-claiming the
    /// index entry. Fails when it already points at another discount.
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
            .routing_key_opt(routing_key)
            .event(&claimed)
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_code_index_any(code).await? else {
                    return Err(DiscountError::CodeTaken);
                };
                if existing.released {
                    existing.write()?.event(&claimed).commit(&self.0).await?;
                    Ok(())
                } else if existing.discount_id == discount_id {
                    Ok(())
                } else {
                    Err(DiscountError::CodeTaken)
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Frees the index entry of a code owned by `discount_id`. A missing or
    /// already freed entry is fine; one owned by another discount is left
    /// alone.
    pub(crate) async fn release_code(
        &self,
        code: &str,
        discount_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), DiscountError> {
        if let Some(index) = self.load_code_index(code).await?
            && index.discount_id == discount_id
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
