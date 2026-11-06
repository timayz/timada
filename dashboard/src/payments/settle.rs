//! Speaks to the provider about an order's money once the order asked for
//! it: takes what is held (`Order::CaptureRequested`), or settles a
//! cancellation (`Order::CancellationRequested`), the hold given back or what
//! was taken refunded first. Whoever wrote the request calls these at once;
//! the sweep ([`PendingPayments`](super::PendingPayments)) calls them again
//! for any the provider could not be asked about then. Each reads the order
//! first and the provider's answer is recorded, so asking twice (two nodes,
//! a retry) does what asking once did.

use timada_orders::{
    CancelPath, HOLD_CANCELLED_REASON, HOLD_EXPIRED_REASON, OrderError, OrderStatus, Released,
};

use super::{Account, CaptureAnswer, DASHBOARD_ACTOR, Payments, ReleaseAnswer};
use crate::cluster::Executor;

/// The orders gateway of one tenant.
type Orders = timada_orders::Command<Executor>;

/// What came of taking an order's held money.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// Taken: the order is paid.
    Paid,
    /// The provider would not take it: recorded on the order, with what it
    /// said.
    Refused(String),
    /// The hold was gone (it lapsed, or was cancelled in the provider's
    /// dashboard): the order is cancelled, nothing taken.
    Released,
    /// The hold was gone and the supplier may have the order: nothing taken,
    /// the hold recorded gone, and the order left open for the merchant.
    KeptOpen,
    /// The provider could not be reached: the request stands, and is asked
    /// again later.
    Unanswered,
    /// No request stands.
    Nothing,
}

/// What came of settling an order's cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Cancelled: the hold given back, or the money refunded.
    Cancelled,
    /// The provider could not be reached, or refused the refund (what it
    /// said): the cancellation stands, and is settled later.
    Unanswered(String),
    /// No cancellation stands.
    Nothing,
}

/// The reason an order is cancelled with when the provider dropped its hold
/// on its own (`lapsed`), or it was cancelled in the provider's dashboard.
pub fn release_reason(lapsed: bool) -> &'static str {
    if lapsed {
        HOLD_EXPIRED_REASON
    } else {
        HOLD_CANCELLED_REASON
    }
}

/// Takes the money a standing capture request asked for.
pub async fn capture(
    payments: &Payments,
    account: &Account,
    orders: &Orders,
    order_id: &str,
) -> anyhow::Result<CaptureOutcome> {
    let Some(order) = orders.load_order(order_id).await? else {
        return Ok(CaptureOutcome::Nothing);
    };
    if order.is_paid() {
        return Ok(CaptureOutcome::Paid);
    }
    let Some(pending) = order.pending_capture else {
        return Ok(CaptureOutcome::Nothing);
    };
    let answer = payments
        .gateway
        .capture_payment(&account.secret_key, &pending.payment_id)
        .await;
    match answer {
        Ok(CaptureAnswer::Captured { amount_cents }) => {
            orders
                .record_payment(DASHBOARD_ACTOR, order_id, &pending.payment_id, amount_cents)
                .await?;
            tracing::info!(tenant = %orders.tenant(), order = %order_id, amount_cents, "an order's payment is taken");
            Ok(CaptureOutcome::Paid)
        }
        Ok(CaptureAnswer::Cancelled { lapsed }) => {
            match orders
                .record_release(
                    DASHBOARD_ACTOR,
                    order_id,
                    &pending.payment_id,
                    release_reason(lapsed),
                )
                .await?
            {
                Released::Cancelled => {
                    tracing::warn!(tenant = %orders.tenant(), order = %order_id, lapsed, "an order's hold was gone when its payment was taken: cancelled");
                    Ok(CaptureOutcome::Released)
                }
                Released::KeptOpen => {
                    tracing::error!(tenant = %orders.tenant(), order = %order_id, lapsed, "an order's hold was gone when its payment was taken, and its supplier may have it: left open for the merchant");
                    Ok(CaptureOutcome::KeptOpen)
                }
            }
        }
        Ok(CaptureAnswer::Refused(reason)) => {
            orders
                .record_capture_failure(DASHBOARD_ACTOR, order_id, &reason)
                .await?;
            tracing::warn!(tenant = %orders.tenant(), order = %order_id, %reason, "the provider would not take an order's payment");
            Ok(CaptureOutcome::Refused(reason))
        }
        Err(err) => {
            tracing::warn!(tenant = %orders.tenant(), order = %order_id, error = %err, "an order's payment could not be taken now: asked again later");
            Ok(CaptureOutcome::Unanswered)
        }
    }
}

/// Settles a standing cancellation: the hold given back, or what was taken
/// refunded (under the key the order fixed), then the order cancelled.
pub async fn settle_cancellation(
    payments: &Payments,
    account: &Account,
    orders: &Orders,
    order_id: &str,
) -> anyhow::Result<CancelOutcome> {
    let Some(order) = orders.load_order(order_id).await? else {
        return Ok(CancelOutcome::Nothing);
    };
    if order.status == OrderStatus::Cancelled {
        return Ok(CancelOutcome::Cancelled);
    }
    let Some(asked) = order.pending_cancellation.clone() else {
        return Ok(CancelOutcome::Nothing);
    };
    if let CancelPath::Release { payment_id } = order.cancel_path() {
        // Nothing is held any more (an order kept open once its hold went
        // back, its supplier perhaps holding it): cancelled without asking
        // the provider anything.
        if order.released_at.is_some() {
            orders
                .record_release(DASHBOARD_ACTOR, order_id, &payment_id, &asked.reason)
                .await?;
            return Ok(CancelOutcome::Cancelled);
        }
        let released = payments
            .gateway
            .release_payment(&account.secret_key, &payment_id)
            .await;
        match released {
            Ok(ReleaseAnswer::Released) => {
                orders
                    .record_release(DASHBOARD_ACTOR, order_id, &payment_id, &asked.reason)
                    .await?;
                return Ok(CancelOutcome::Cancelled);
            }
            // Taken in between, in the provider's own dashboard: refunded
            // like any taken payment.
            Ok(ReleaseAnswer::Captured { amount_cents }) => {
                orders
                    .record_payment(DASHBOARD_ACTOR, order_id, &payment_id, amount_cents)
                    .await?;
            }
            Err(err) => {
                tracing::warn!(tenant = %orders.tenant(), order = %order_id, error = %err, "an order's hold could not be given back now: tried again later");
                return Ok(CancelOutcome::Unanswered(err.to_string()));
            }
        }
    }
    let order = orders
        .load_order(order_id)
        .await?
        .ok_or(OrderError::OrderNotFound)?;
    let outstanding = order.outstanding_cents();
    if outstanding > 0 || order.pending_refund.is_some() {
        let pending = orders
            .request_refund(&asked.requested_by, order_id, outstanding)
            .await?;
        let refunded = payments
            .gateway
            .refund(
                &account.secret_key,
                &pending.payment_id,
                pending.amount_cents,
                &pending.idempotency_key,
            )
            .await;
        let refund_id = match refunded {
            Ok(refund_id) => refund_id,
            Err(err) => {
                tracing::warn!(tenant = %orders.tenant(), order = %order_id, error = %err, "an order's refund could not be made now: tried again later");
                return Ok(CancelOutcome::Unanswered(err.to_string()));
            }
        };
        orders
            .record_refund(DASHBOARD_ACTOR, order_id, &refund_id, pending.amount_cents)
            .await?;
    }
    orders.complete_cancellation(order_id).await?;
    Ok(CancelOutcome::Cancelled)
}
