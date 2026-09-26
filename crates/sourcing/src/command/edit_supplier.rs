use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{SupplierRenamed, SupplierResumed, SupplierSuspended},
    error::SourcingError,
};

impl<E: Executor> super::Command<'_, E> {
    pub async fn rename_supplier(
        &self,
        id: impl Into<String>,
        name: String,
    ) -> Result<(), SourcingError> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(SourcingError::Required("name"));
        }
        let supplier = self.require_supplier(id).await?;
        if supplier.name == name {
            return Ok(());
        }

        supplier
            .write()?
            .event(&SupplierRenamed { name })
            .commit(self.executor)
            .await?;
        Ok(())
    }

    /// Stops buying and re-pricing from a supplier without unpicking what it
    /// sources: the links, the rules and the prices stay as they are.
    pub async fn suspend_supplier(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), SourcingError> {
        let supplier = self.require_supplier(id).await?;
        if supplier.suspended {
            return Ok(());
        }

        supplier
            .write()?
            .event(&SupplierSuspended { reason })
            .commit(self.executor)
            .await?;
        tracing::info!(supplier_id = %supplier.id, "supplier suspended");
        Ok(())
    }

    pub async fn resume_supplier(&self, id: impl Into<String>) -> Result<(), SourcingError> {
        let supplier = self.require_supplier(id).await?;
        if !supplier.suspended {
            return Ok(());
        }

        supplier
            .write()?
            .event(&SupplierResumed)
            .commit(self.executor)
            .await?;
        Ok(())
    }
}
