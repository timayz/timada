use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::StockLevelSynced, error::InventoryError};

impl<E: Executor> super::Command<'_, E> {
    /// Sets how many units can still be sold, as an absolute figure — what a
    /// supplier's feed says it holds, or what a stock-take counted.
    ///
    /// There is no zero guard: a supplier being out of stock is the whole
    /// point. Nothing is written when the level is already that, so a feed
    /// polled every hour appends no event while nothing moves; the `bool`
    /// says whether it did.
    pub async fn sync_stock_level(
        &self,
        id: impl Into<String>,
        available: u32,
    ) -> Result<bool, InventoryError> {
        let item = self.require_stock_item(id).await?;
        if item.available() == available {
            return Ok(false);
        }

        item.write()?
            .event(&StockLevelSynced { available })
            .commit(self.0)
            .await?;
        Ok(true)
    }
}
