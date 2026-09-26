use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{ProductSourced, SourcingStopped},
    error::SourcingError,
};

use super::sourced_product_id;

#[derive(Debug, Clone)]
pub struct SourceProduct {
    /// One of the shop's products, by its catalogue id.
    pub product_id: String,
    pub supplier_id: String,
    pub external_item_id: String,
    /// The supplier's own variant inside the item; `None` when it has one.
    pub external_sku: Option<String>,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Says where a product is bought.
    ///
    /// Idempotent, and a move at the same time: sourcing a product that is
    /// already sourced from the same item changes nothing, from another it
    /// records the move on the same stream — which is what keeps a product
    /// to one supplier at a time without a uniqueness index anywhere.
    pub async fn source_product(
        &self,
        cmd: SourceProduct,
        routing_key: Option<String>,
    ) -> Result<String, SourcingError> {
        if cmd.product_id.trim().is_empty() {
            return Err(SourcingError::Required("product_id"));
        }
        let external_item_id = cmd.external_item_id.trim().to_owned();
        if external_item_id.is_empty() {
            return Err(SourcingError::Required("external_item_id"));
        }
        let external_sku = cmd
            .external_sku
            .map(|sku| sku.trim().to_owned())
            .filter(|sku| !sku.is_empty());

        let supplier = self.require_supplier(&cmd.supplier_id).await?;
        if supplier.suspended {
            return Err(SourcingError::SupplierSuspended);
        }

        let sourced = ProductSourced {
            supplier_id: supplier.id.clone(),
            product_id: cmd.product_id.trim().to_owned(),
            external_item_id,
            external_sku,
        };
        let id = sourced_product_id(&sourced.product_id);
        let result = evento::append(&id)
            .routing_key_opt(routing_key.clone())
            .event(&sourced)
            .commit(self.executor)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(sourced_id = %id, product_id = %sourced.product_id, "product sourced");
                return Ok(id);
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }

        // It has been sourced before. Saying the same thing again writes
        // nothing; saying something else is a move.
        let known = self
            .load_sourced(&id)
            .await?
            .ok_or(SourcingError::NotSourced)?;
        if known.active
            && known.supplier_id == sourced.supplier_id
            && known.external_item_id == sourced.external_item_id
            && known.external_sku == sourced.external_sku
        {
            return Ok(id);
        }

        known
            .write()?
            .routing_key_opt(routing_key)
            .event(&sourced)
            .commit(self.executor)
            .await?;
        tracing::info!(sourced_id = %id, supplier_id = %sourced.supplier_id, "product re-sourced");
        Ok(id)
    }

    /// The shop buys this product somewhere else, or no longer at all. The
    /// price it is on sale at is left exactly as it is: withdrawing it is a
    /// catalogue decision, not a consequence of losing a supplier.
    pub async fn stop_sourcing(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), SourcingError> {
        let sourced = self.require_sourced(id).await?;

        sourced
            .write()?
            .event(&SourcingStopped { reason })
            .commit(self.executor)
            .await?;
        tracing::info!(sourced_id = %sourced.id, "sourcing stopped");
        Ok(())
    }
}
