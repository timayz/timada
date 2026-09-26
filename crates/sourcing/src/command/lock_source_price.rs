use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{SourcePriceLocked, SourcePriceUnlocked},
    error::SourcingError,
};

impl<E: Executor> super::Command<'_, E> {
    /// The operator decided this price. The sync leaves it alone — and does
    /// not queue it for review either: somebody who locked a price does not
    /// want to be asked about it every few hours.
    pub async fn lock_source_price(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), SourcingError> {
        let sourced = self.require_sourced(id).await?;
        if sourced.locked {
            return Ok(());
        }

        sourced
            .write()?
            .event(&SourcePriceLocked { reason })
            .commit(self.executor)
            .await?;
        Ok(())
    }

    pub async fn unlock_source_price(&self, id: impl Into<String>) -> Result<(), SourcingError> {
        let sourced = self.require_sourced(id).await?;
        if !sourced.locked {
            return Ok(());
        }

        sourced
            .write()?
            .event(&SourcePriceUnlocked)
            .commit(self.executor)
            .await?;
        Ok(())
    }
}
