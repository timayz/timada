use evento::{Executor, ProjectionAggregate};
use timada_tax::PinnedRate;

use crate::{aggregator::OrderRatePinned, error::OrderError};

use super::place_order::rate_fits;

impl<E: Executor> super::Command<'_, E> {
    /// Pins the rate an order in another currency goes to the books at, when
    /// none could be had while it was placed (the source was down). The first
    /// rate stays: returns `false` when the order already has one.
    pub async fn pin_exchange_rate(
        &self,
        id: impl Into<String>,
        rate: PinnedRate,
    ) -> Result<bool, OrderError> {
        let order = self.load_existing(id).await?;
        if order.rate_pinned {
            return Ok(false);
        }
        rate_fits(&rate, &order.currency)?;
        order
            .write()?
            .event(&OrderRatePinned { rate })
            .commit(self.0)
            .await?;
        tracing::info!(order_id = %order.id, "exchange rate pinned");
        Ok(true)
    }
}
