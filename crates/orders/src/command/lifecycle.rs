use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Cancelled, Fulfilled, Paid};
use crate::error::OrdersError;
use crate::value_object::{normalize_reason, normalize_tracking};

use super::OrderStatus;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records the payment of an order awaiting it; already paid is a no-op.
    pub async fn mark_order_paid(
        &self,
        actor: &str,
        order_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrdersError> {
        let order = self.require_order(order_id).await?;
        match order.status {
            OrderStatus::AwaitingPayment => {}
            OrderStatus::Paid => return Ok(()),
            OrderStatus::Fulfilled | OrderStatus::Cancelled => {
                return Err(OrdersError::NotAwaitingPayment);
            }
        }
        order
            .write()?
            .routing_key_opt(routing_key)
            .event(&Paid {
                paid_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Records that a paid order left, with its tracking number if any;
    /// already fulfilled is a no-op.
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
            OrderStatus::AwaitingPayment | OrderStatus::Cancelled => {
                return Err(OrdersError::NotPaid);
            }
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
            OrderStatus::AwaitingPayment | OrderStatus::Paid => {}
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
}
