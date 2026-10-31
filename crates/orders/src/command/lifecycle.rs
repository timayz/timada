use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Cancelled, Fulfilled, RefundRequested, Refunded};
use crate::error::OrderError;
use crate::value_object::{normalize_reason, normalize_tracking};

use super::{OrderStatus, PendingRefund};

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
    ) -> Result<(), OrderError> {
        let tracking = normalize_tracking(tracking.as_deref())?;
        let order = self.require_order(order_id).await?;
        match order.status {
            OrderStatus::Paid => {}
            OrderStatus::Fulfilled => return Ok(()),
            OrderStatus::Cancelled => return Err(OrderError::OrderCancelled),
        }
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&Fulfilled {
                tracking,
                fulfilled_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Calls off an order that has not left; already cancelled is a no-op.
    /// Giving the money back is the caller's (it speaks to the provider):
    /// asked for with [`request_refund`](Self::request_refund), recorded
    /// with [`record_refund`](Self::record_refund). While a refund is asked
    /// for and not recorded, the order is not cancelled (`RefundPending`):
    /// the money must be known to be on its way back first.
    pub async fn cancel_order(
        &self,
        actor: &str,
        order_id: &str,
        reason: String,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let reason = normalize_reason(&reason)?;
        let order = self.require_order(order_id).await?;
        match order.status {
            OrderStatus::Paid => {}
            OrderStatus::Cancelled => return Ok(()),
            OrderStatus::Fulfilled => return Err(OrderError::NotCancellable),
        }
        if order.pending_refund.is_some() {
            return Err(OrderError::RefundPending);
        }
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&Cancelled {
                reason,
                cancelled_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Asks for `amount_cents` of the order's payment to go back, before the
    /// provider is: the key the provider must be asked with comes back, and
    /// asking again while the refund is pending gives the same key, so a
    /// retry (or another node) makes the same refund. The order must be paid
    /// and not left (`NotCancellable`), and the amount within what is still
    /// outstanding (`InvalidRefund`). Settled by
    /// [`record_refund`](Self::record_refund).
    pub async fn request_refund(
        &self,
        actor: &str,
        order_id: &str,
        amount_cents: u64,
        routing_key: Option<String>,
    ) -> Result<PendingRefund, OrderError> {
        let order = self.require_order(order_id).await?;
        if let Some(pending) = &order.pending_refund {
            return Ok(pending.clone());
        }
        if order.status == OrderStatus::Fulfilled {
            return Err(OrderError::NotCancellable);
        }
        if amount_cents == 0 || amount_cents > order.outstanding_cents() {
            return Err(OrderError::InvalidRefund);
        }
        let idempotency_key = format!("timada-refund-{order_id}-{}", order.refund_ids.len() + 1);
        let requested = RefundRequested {
            payment_id: order.payment.payment_id.clone(),
            amount_cents,
            idempotency_key: idempotency_key.clone(),
            requested_by: actor.to_owned(),
        };
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&requested)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(self
            .require_order(order_id)
            .await?
            .pending_refund
            .unwrap_or(PendingRefund {
                payment_id: requested.payment_id,
                amount_cents,
                idempotency_key,
                requested_at: 0,
            }))
    }

    /// Records money the provider gave back for the order's payment; the same
    /// refund recorded twice is a no-op. Settles the pending request, if any.
    pub async fn record_refund(
        &self,
        actor: &str,
        order_id: &str,
        refund_id: &str,
        amount_cents: u64,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
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
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
