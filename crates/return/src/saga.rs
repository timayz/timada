//! The `return-flow` subscription: approval → refund → refunded.
//!
//! Like the order fulfillment saga, this reacts to events across aggregates
//! (its own `Return` plus the payment context's `ChargeRefunded`), so it is
//! deliberately **not** `.strict()` — without strict mode evento derives its
//! read filter from the registered handlers, which is exactly the subset this
//! flow cares about.
//!
//! Idempotency rests on the same three legs as the fulfillment saga:
//! `timada_payment::refund` is a no-op unless the charge is `Captured`, the
//! `ReturnRefunded` append goes through the replayed view's optimistic
//! concurrency, and the status guard skips redelivered events. A refund that
//! did *not* come from a return — the fulfillment saga's supplier-rejection
//! compensation refunds too — finds no return aggregate and is left alone.

use std::sync::Arc;

use evento::ProjectionAggregate as _;
use evento::metadata::Event;
use evento::subscription::{Context, SubscriptionBuilder};
use timada_core::Executor;
use timada_order::load_order;
use timada_payment::{ChargeRefunded, PaymentProvider, load_payment, refund};

use crate::aggregate::{ReturnApproved, ReturnRefunded};
use crate::commands::return_id;
use crate::view::{ReturnStatus, load_return};

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const RETURN_FLOW_SUBSCRIPTION: &str = "return-flow";

/// The return-flow subscription, unstarted — tests drive it with
/// `.no_retry().run_once(&executor)`.
pub fn return_flow_subscription(
    executor: Executor,
    provider: Arc<dyn PaymentProvider>,
) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(RETURN_FLOW_SUBSCRIPTION)
        .data(executor)
        .data(provider)
        .handler(on_return_approved())
        .handler(on_charge_refunded())
}

fn executor<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<Executor> {
    ctx.get::<Executor>().ok_or_else(|| {
        anyhow::anyhow!("`{RETURN_FLOW_SUBSCRIPTION}` subscription was started without an executor")
    })
}

fn provider<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<Arc<dyn PaymentProvider>> {
    ctx.get::<Arc<dyn PaymentProvider>>().ok_or_else(|| {
        anyhow::anyhow!("`{RETURN_FLOW_SUBSCRIPTION}` subscription was started without a provider")
    })
}

/// An approved return refunds the order's charge. Every missing read bails so
/// the retry covers a race with a concurrent write.
#[evento::subscription]
async fn on_return_approved<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnApproved>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    let Some(current) = load_return(&executor, &event.aggregate_id).await? else {
        anyhow::bail!("approved return {} not found", event.aggregate_id);
    };
    let Some(order) = load_order(&executor, &current.order_id).await? else {
        anyhow::bail!("order {} of an approved return not found", current.order_id);
    };
    let Some(payment_id) = order.payment_id.as_deref() else {
        anyhow::bail!("delivered order {} has no payment to refund", order.id);
    };

    refund(&executor, &provider(ctx)?, payment_id).await
}

/// The provider confirmed a refund — if an approved return asked for it, the
/// return is done. Refunds with no return (saga compensation) pass through.
#[evento::subscription]
async fn on_charge_refunded<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ChargeRefunded>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    let Some(payment) = load_payment(&executor, &event.aggregate_id).await? else {
        anyhow::bail!("refunded payment {} not found", event.aggregate_id);
    };
    let Some(current) = load_return(&executor, &return_id(&payment.order_id)).await? else {
        tracing::debug!(
            payment_id = %payment.id,
            order_id = %payment.order_id,
            "refund without a return — compensation, not RMA"
        );
        return Ok(());
    };
    if current.status != ReturnStatus::Approved {
        tracing::debug!(
            return_id = %current.id,
            status = current.status.as_str(),
            "refund arrived for a return that is not awaiting one"
        );
        return Ok(());
    }

    current
        .write()?
        .event(&ReturnRefunded {
            order_id: payment.order_id.clone(),
            payment_id: payment.id.clone(),
        })
        .commit(&executor)
        .await?;

    tracing::info!(order_id = %payment.order_id, "return refunded");
    Ok(())
}
