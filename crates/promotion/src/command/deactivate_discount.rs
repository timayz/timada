use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::DiscountDeactivated, error::PromotionError};

impl<E: Executor> super::Command<'_, E> {
    /// Stops a code from being redeemed. A no-op when already inactive.
    pub async fn deactivate_discount(&self, id: impl Into<String>) -> Result<(), PromotionError> {
        let Some(discount) = self.load_discount(id).await? else {
            return Err(PromotionError::UnknownCode);
        };
        if !discount.active {
            return Ok(());
        }

        discount
            .write()?
            .event(&DiscountDeactivated)
            .commit(self.executor)
            .await?;
        tracing::info!(discount_id = %discount.id, "discount deactivated");
        Ok(())
    }
}
