use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::DiscountRedemptionReleased, error::PromotionError};

use super::discount_id;

impl<E: Executor> super::Command<'_, E> {
    /// Frees the slot an order held on a code (the order fell through).
    /// Returns whether there was one; a repeat is a no-op.
    pub async fn release_discount(
        &self,
        code: &str,
        order_id: &str,
    ) -> Result<bool, PromotionError> {
        let Some(discount) = self.load_discount(discount_id(code)).await? else {
            return Err(PromotionError::UnknownCode);
        };
        let deleted =
            sqlx::query("DELETE FROM promotion_redemption WHERE code = ? AND order_id = ?")
                .bind(&discount.code)
                .bind(order_id)
                .execute(&self.db)
                .await?
                .rows_affected();
        if deleted == 0 {
            return Ok(false);
        }

        discount
            .write()?
            .event(&DiscountRedemptionReleased {
                order_id: order_id.to_owned(),
            })
            .commit(self.executor)
            .await?;
        tracing::info!(discount_id = %discount.id, %order_id, "discount redemption released");
        Ok(true)
    }
}
