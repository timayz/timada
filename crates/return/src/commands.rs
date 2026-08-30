//! Write-side commands for [`Return`](crate::aggregate::Return).

use evento::ProjectionAggregate as _;
use timada_core::Executor;
use timada_order::load_order;

use crate::aggregate::{ReturnApproved, ReturnRejected, ReturnRequested};
use crate::state::ReturnPolicy;
use crate::view::{ReturnStatus, load_return};

/// The id of the return for `order_id` — one return per order by
/// construction, so "does this order have a return?" is one lookup.
pub fn return_id(order_id: &str) -> String {
    evento::hash_ids(vec![order_id, "return"])
}

/// Why a return request was refused.
#[derive(Debug, thiserror::Error)]
pub enum RequestReturnError {
    #[error("this order does not exist")]
    UnknownOrder,
    #[error("only a delivered order can be returned")]
    NotDelivered,
    #[error("the return window for this order has closed")]
    WindowClosed,
    #[error("a return for this order is already underway")]
    AlreadyOpen,
    #[error("this order was already refunded")]
    AlreadyRefunded,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

/// Open (or, after a rejection, re-open) the return conversation for an order.
/// Returns the return aggregate id.
#[tracing::instrument(skip(executor, policy))]
pub async fn request_return(
    executor: &Executor,
    policy: &ReturnPolicy,
    order_id: &str,
    reason: &str,
) -> Result<String, RequestReturnError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(RequestReturnError::Invalid(
            "tell us why you are returning the order".to_owned(),
        ));
    }

    let Some(order) = load_order(executor, order_id).await? else {
        return Err(RequestReturnError::UnknownOrder);
    };
    if !order.is_delivered() {
        return Err(RequestReturnError::NotDelivered);
    }
    let delivered_at = order
        .delivered_at
        .ok_or_else(|| anyhow::anyhow!("delivered order {order_id} has no delivery timestamp"))?;
    if timada_core::now_millis() > delivered_at.saturating_add(policy.window_millis()) {
        return Err(RequestReturnError::WindowClosed);
    }

    let id = return_id(order_id);
    let event = ReturnRequested {
        order_id: order_id.to_owned(),
        reason: reason.to_owned(),
    };
    match load_return(executor, &id).await? {
        // A rejected return may be argued again; anything else is in flight
        // or finished.
        Some(current) => match current.status {
            ReturnStatus::Rejected => {
                current
                    .write()?
                    .event(&event)
                    .commit(executor)
                    .await
                    .map_err(anyhow::Error::from)?;
            }
            ReturnStatus::Refunded => return Err(RequestReturnError::AlreadyRefunded),
            ReturnStatus::Requested | ReturnStatus::Approved => {
                return Err(RequestReturnError::AlreadyOpen);
            }
        },
        None => {
            evento::append(&id)
                .original_version(0)
                .event(&event)
                .commit(executor)
                .await
                .map_err(anyhow::Error::from)?;
        }
    }

    tracing::info!(order_id, return_id = %id, "return requested");
    Ok(id)
}

/// Accept a requested return. Any other state is a logged no-op — a
/// double-clicked Approve button should not produce an error page, and the
/// refund itself is the `return-flow` subscription's job.
#[tracing::instrument(skip(executor))]
pub async fn approve_return(executor: &Executor, return_id: &str) -> anyhow::Result<()> {
    let Some(current) = load_return(executor, return_id).await? else {
        tracing::warn!(return_id, "cannot approve an unknown return");
        return Ok(());
    };
    if current.status != ReturnStatus::Requested {
        tracing::info!(
            return_id,
            status = current.status.as_str(),
            "not approvable"
        );
        return Ok(());
    }

    current
        .write()?
        .event(&ReturnApproved)
        .commit(executor)
        .await?;

    tracing::info!(return_id, "return approved");
    Ok(())
}

/// Decline a requested return, with a reason the customer sees.
#[tracing::instrument(skip(executor))]
pub async fn reject_return(
    executor: &Executor,
    return_id: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let Some(current) = load_return(executor, return_id).await? else {
        tracing::warn!(return_id, "cannot reject an unknown return");
        return Ok(());
    };
    if current.status != ReturnStatus::Requested {
        tracing::info!(
            return_id,
            status = current.status.as_str(),
            "not rejectable"
        );
        return Ok(());
    }

    let reason = reason.trim();
    let reason = if reason.is_empty() {
        "not accepted"
    } else {
        reason
    };

    current
        .write()?
        .event(&ReturnRejected {
            reason: reason.to_owned(),
        })
        .commit(executor)
        .await?;

    tracing::info!(return_id, "return rejected");
    Ok(())
}
