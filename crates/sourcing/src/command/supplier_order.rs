use evento::{Executor, ProjectionAggregate};
use timada_core::{Address, Money};

use crate::{
    aggregator::{
        SupplierOrderCancelled, SupplierOrderDrafted, SupplierOrderPlaced,
        SupplierOrderRecordedByHand, SupplierOrderRefused, SupplierOrderShipped,
    },
    error::SourcingError,
    value_object::{SupplierOrderLine, SupplierOrderStatus},
};

use super::supplier_order_id;

#[derive(Debug, Clone)]
pub struct DraftSupplierOrder {
    pub order_id: String,
    pub supplier_id: String,
    pub lines: Vec<SupplierOrderLine>,
    pub ship_to: Address,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Writes down what the shop means to buy from one supplier for one of its
    /// orders. The id derives from the pair, so a redelivered `OrderPaid`
    /// drafts nothing twice and an order split across two suppliers gets two
    /// purchases without anybody special-casing it.
    pub async fn draft_supplier_order(
        &self,
        cmd: DraftSupplierOrder,
        routing_key: Option<String>,
    ) -> Result<String, SourcingError> {
        if cmd.order_id.trim().is_empty() {
            return Err(SourcingError::Required("order_id"));
        }
        if cmd.lines.is_empty() {
            return Err(SourcingError::NothingToOrder);
        }
        let supplier = self.require_supplier(&cmd.supplier_id).await?;
        let mut cost = Money::zero(
            cmd.lines
                .first()
                .map(|line| line.unit_cost.currency.as_str())
                .unwrap_or(Money::EUR),
        );
        for line in &cmd.lines {
            cost = cost.checked_add(&line.total()?)?;
        }

        let id = supplier_order_id(&cmd.order_id, &supplier.id);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&SupplierOrderDrafted {
                order_id: cmd.order_id.clone(),
                supplier_id: supplier.id,
                lines: cmd.lines,
                ship_to: cmd.ship_to,
                cost,
            })
            .commit(self.executor)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(purchase_id = %id, order_id = %cmd.order_id, "purchase drafted");
                Ok(id)
            }
            // Already drafted: the same purchase, not a second one.
            Err(evento::WriteError::InvalidOriginalVersion) => Ok(id),
            Err(err) => Err(err.into()),
        }
    }

    /// The supplier took the order. Idempotent for the same reference, so a
    /// worker that died after the supplier answered records it once.
    pub async fn record_supplier_order_placed(
        &self,
        id: impl Into<String>,
        external_order_id: String,
        cost: Money,
    ) -> Result<(), SourcingError> {
        let external_order_id = external_order_id.trim().to_owned();
        if external_order_id.is_empty() {
            return Err(SourcingError::Required("external_order_id"));
        }
        let purchase = self.require_purchase(id).await?;
        if purchase.external_order_id.as_deref() == Some(external_order_id.as_str()) {
            return Ok(());
        }
        if purchase.status != SupplierOrderStatus::Drafted {
            return Err(SourcingError::SupplierOrderNotDraft);
        }

        purchase
            .write()?
            .event(&SupplierOrderPlaced {
                external_order_id: external_order_id.clone(),
                cost,
            })
            .commit(self.executor)
            .await?;
        tracing::info!(purchase_id = %purchase.id, %external_order_id, "purchase placed");
        Ok(())
    }

    /// Bought on the supplier's own site: the operator types the reference in.
    /// The way a supplier with no API is worked, and the way out when its API
    /// will not take an order it would take by hand.
    pub async fn record_supplier_order_by_hand(
        &self,
        id: impl Into<String>,
        external_order_id: String,
        note: String,
    ) -> Result<(), SourcingError> {
        let external_order_id = external_order_id.trim().to_owned();
        if external_order_id.is_empty() {
            return Err(SourcingError::Required("external_order_id"));
        }
        let purchase = self.require_purchase(id).await?;
        if purchase.external_order_id.as_deref() == Some(external_order_id.as_str()) {
            return Ok(());
        }
        // A refused purchase may be bought by hand after all; a shipped one
        // is past arguing about.
        if !matches!(
            purchase.status,
            SupplierOrderStatus::Drafted | SupplierOrderStatus::Refused
        ) {
            return Err(SourcingError::SupplierOrderNotDraft);
        }

        purchase
            .write()?
            .event(&SupplierOrderRecordedByHand {
                external_order_id,
                note,
            })
            .commit(self.executor)
            .await?;
        Ok(())
    }

    pub async fn refuse_supplier_order(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), SourcingError> {
        let purchase = self.require_purchase(id).await?;
        if purchase.status == SupplierOrderStatus::Refused {
            return Ok(());
        }
        if purchase.status != SupplierOrderStatus::Drafted {
            return Err(SourcingError::SupplierOrderNotDraft);
        }

        purchase
            .write()?
            .event(&SupplierOrderRefused {
                reason: reason.clone(),
            })
            .commit(self.executor)
            .await?;
        tracing::warn!(purchase_id = %purchase.id, %reason, "supplier would not take the order");
        Ok(())
    }

    /// The supplier handed the parcel to a carrier. This is what dispatches
    /// the shop's own shipment, through the process manager.
    pub async fn record_supplier_order_shipped(
        &self,
        id: impl Into<String>,
        carrier: String,
        tracking_number: String,
    ) -> Result<(), SourcingError> {
        let tracking_number = tracking_number.trim().to_owned();
        if tracking_number.is_empty() {
            return Err(SourcingError::Required("tracking_number"));
        }
        let purchase = self.require_purchase(id).await?;
        if purchase.status == SupplierOrderStatus::Shipped {
            return Ok(());
        }
        if purchase.status != SupplierOrderStatus::Placed {
            return Err(SourcingError::SupplierOrderNotPlaced);
        }

        purchase
            .write()?
            .event(&SupplierOrderShipped {
                carrier,
                tracking_number: tracking_number.clone(),
            })
            .commit(self.executor)
            .await?;
        tracing::info!(purchase_id = %purchase.id, %tracking_number, "supplier shipped");
        Ok(())
    }

    /// Calls a purchase off. A parcel already on its way cannot be, so that
    /// is refused rather than pretended.
    pub async fn cancel_supplier_order(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), SourcingError> {
        let purchase = self.require_purchase(id).await?;
        if purchase.status == SupplierOrderStatus::Cancelled {
            return Ok(());
        }
        if purchase.status == SupplierOrderStatus::Shipped {
            return Err(SourcingError::SupplierOrderShipped);
        }

        purchase
            .write()?
            .event(&SupplierOrderCancelled { reason })
            .commit(self.executor)
            .await?;
        Ok(())
    }
}
