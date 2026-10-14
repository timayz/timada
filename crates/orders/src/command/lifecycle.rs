use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Cancelled, Fulfilled, Refunded};
use crate::error::OrdersError;
use crate::value_object::{normalize_reason, normalize_tracking};

use super::OrderStatus;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records that an order left, with its tracking number if any; already
    /// fulfilled is a no-op.
    pub async fn fulfil_order(
        &self,
        actor: &str,
        order_id: &str,
        tracking: Option<String>,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        let tracking = normalize_tracking(tracking.as_deref())?;
        let order = self.require_order(order_id).await?;
        match order.status {
            OrderStatus::Paid => {}
            OrderStatus::Fulfilled => return Ok(()),
            OrderStatus::Cancelled => return Err(OrdersError::OrderCancelled),
        }
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&Fulfilled {
                tracking,
                fulfilled_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Calls off an order that has not left; already cancelled is a no-op.
    /// Giving the money back is the caller's (it speaks to the provider),
    /// recorded with [`record_refund`](Self::record_refund).
    pub async fn cancel_order(
        &self,
        actor: &str,
        order_id: &str,
        reason: String,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        let reason = normalize_reason(&reason)?;
        let order = self.require_order(order_id).await?;
        match order.status {
            OrderStatus::Paid => {}
            OrderStatus::Cancelled => return Ok(()),
            OrderStatus::Fulfilled => return Err(OrdersError::NotCancellable),
        }
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&Cancelled {
                reason,
                cancelled_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Records money the provider gave back for the order's payment; the same
    /// refund recorded twice is a no-op.
    pub async fn record_refund(
        &self,
        actor: &str,
        order_id: &str,
        refund_id: &str,
        amount_cents: u64,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        let order = self.require_order(order_id).await?;
        if order.refund_ids.iter().any(|id| id == refund_id) {
            return Ok(());
        }
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&Refunded {
                payment_id: order.payment.payment_id.clone(),
                refund_id: refund_id.to_owned(),
                amount_cents,
                refunded_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
