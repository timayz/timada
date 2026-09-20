//! The one way the fulfillment saga could wait forever: a payment that is
//! requested and then never captured nor declined (an abandoned card form, a
//! PSP callback that never comes) keeps the order's stock reserved.
//!
//! evento has no timers, so the deadline is kept in SQL: a subscription lists
//! the sagas waiting for their payment, and a periodic sweep declines the
//! payments that waited too long. From there the ordinary declined-payment
//! path compensates — stock released, order cancelled — and a capture that
//! still comes in afterwards is refused by the payment context.

use std::{sync::Arc, time::Duration};

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_payment::{CancelOutcome, PaymentError, PaymentProvider, PaymentStatus};

use crate::{
    aggregator::{
        FulfillmentCompensated, FulfillmentCompleted, FulfillmentStarted, LineStockReserved,
        PaymentCaptured, PaymentRequested, PaymentWaived, ShipmentRequested,
    },
    saga::load_fulfillment,
    value_object::FulfillmentStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const PAYMENT_DEADLINE_SUBSCRIPTION: &str = "order-payment-deadline";

/// The decline reason — and then the order's cancellation reason — of a
/// payment that waited too long.
pub const PAYMENT_TIMED_OUT: &str = "payment timed out";

/// How to word a cancellation reason for the customer: the reasons the saga
/// gives itself are translated, an operator's free text is shown as typed.
pub fn cancellation_reason_label(reason: &str) -> &str {
    match reason {
        PAYMENT_TIMED_OUT => "paiement non finalisé dans les délais",
        "payment declined" => "paiement refusé",
        "out of stock" => "produit indisponible",
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AwaitingPaymentRow {
    pub order_id: String,
    pub payment_id: String,
    /// Unix seconds of the payment request.
    pub since: i64,
}

pub fn payment_deadline_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PAYMENT_DEADLINE_SUBSCRIPTION)
        .handler(insert_on_payment_requested())
        .handler(remove_on_payment_captured())
        .handler(remove_on_fulfillment_compensated())
        .skip::<FulfillmentStarted>()
        .skip::<LineStockReserved>()
        .skip::<PaymentWaived>()
        .skip::<ShipmentRequested>()
        .skip::<FulfillmentCompleted>()
        .strict()
}

/// The orders whose payment was requested at or before `requested_before`
/// (Unix seconds) and is still awaited, oldest first.
pub async fn orders_awaiting_payment(
    db: &SqlitePool,
    requested_before: u64,
) -> sqlx::Result<Vec<AwaitingPaymentRow>> {
    sqlx::query_as(
        "SELECT order_id, payment_id, since FROM order_awaiting_payment
         WHERE since <= ?
         ORDER BY since, order_id",
    )
    .bind(requested_before as i64)
    .fetch_all(db)
    .await
}

/// Declines every payment requested at or before `requested_before` that is
/// still pending — after calling its session off at the `provider` — and
/// returns how many. Safe to repeat and to race with a
/// capture: only a payment still `Requested` is declined, and the saga only
/// compensates an order still awaiting that payment.
pub async fn expire_unpaid_orders<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    provider: &dyn PaymentProvider,
    requested_before: u64,
) -> anyhow::Result<u32> {
    let mut expired = 0;
    for row in orders_awaiting_payment(db, requested_before).await? {
        let awaiting = load_fulfillment(executor, &row.order_id)
            .await?
            .is_some_and(|saga| {
                saga.status == FulfillmentStatus::AwaitingPayment
                    && saga.payment_id.as_deref() == Some(&row.payment_id)
            });
        if !awaiting {
            // The list trails the saga by one subscription; the row goes
            // away on its own.
            continue;
        }
        let pending = timada_payment::load_payment(executor, &row.payment_id)
            .await?
            .is_some_and(|p| p.status == PaymentStatus::Requested);
        if !pending {
            continue;
        }
        // The provider's session goes first, so nobody pays an order that is
        // being cancelled. Paid in the meantime: that is a capture, not a
        // timeout.
        if let CancelOutcome::AlreadyPaid { reference } =
            timada_payment::cancel_payment_session(db, provider, &row.payment_id).await?
        {
            match timada_payment::Command(executor)
                .capture_payment(&row.payment_id, reference)
                .await
            {
                Ok(()) | Err(PaymentError::NotRequested) => {}
                Err(err) => return Err(err.into()),
            }
            tracing::info!(order_id = %row.order_id, "paid just before the timeout: captured");
            continue;
        }
        match timada_payment::Command(executor)
            .decline_payment(&row.payment_id, PAYMENT_TIMED_OUT.to_owned())
            .await
        {
            Ok(()) => {
                tracing::warn!(order_id = %row.order_id, "payment timed out");
                expired += 1;
            }
            // Captured or declined in between: nothing left to expire.
            Err(PaymentError::NotRequested) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(expired)
}

/// Runs [`expire_unpaid_orders`] every `every`, forever, for payments older
/// than `timeout`: spawn it once per deployment.
pub async fn run_payment_timeouts<E: Executor>(
    executor: E,
    db: SqlitePool,
    provider: Arc<dyn PaymentProvider>,
    timeout: Duration,
    every: Duration,
) {
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let swept = match timada_core::time::now_unix_secs() {
            Ok(now) => {
                expire_unpaid_orders(
                    &executor,
                    &db,
                    provider.as_ref(),
                    now.saturating_sub(timeout.as_secs()),
                )
                .await
            }
            Err(err) => Err(err),
        };
        if let Err(err) = swept {
            tracing::error!(error = %err, "payment timeout sweep failed");
        }
    }
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

async fn remove<E: Executor>(ctx: &Context<'_, E>, fulfillment_id: &str) -> anyhow::Result<()> {
    let Some(saga) = crate::saga::load_fulfillment_by_id(ctx.executor, fulfillment_id).await?
    else {
        return Ok(());
    };
    sqlx::query("DELETE FROM order_awaiting_payment WHERE order_id = ?")
        .bind(&saga.order_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn insert_on_payment_requested<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<PaymentRequested>,
) -> anyhow::Result<()> {
    let Some(saga) = crate::saga::load_fulfillment_by_id(ctx.executor, &event.aggregate_id).await?
    else {
        anyhow::bail!("fulfillment {} cannot be loaded", event.aggregate_id);
    };
    // Replayed after the fact: the wait is already over.
    if saga.status != FulfillmentStatus::AwaitingPayment {
        return Ok(());
    }
    sqlx::query(
        "INSERT OR IGNORE INTO order_awaiting_payment (order_id, payment_id, since)
         VALUES (?, ?, ?)",
    )
    .bind(&saga.order_id)
    .bind(&event.data.payment_id)
    .bind(event.timestamp as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn remove_on_payment_captured<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<PaymentCaptured>,
) -> anyhow::Result<()> {
    remove(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn remove_on_fulfillment_compensated<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<FulfillmentCompensated>,
) -> anyhow::Result<()> {
    remove(ctx, &event.aggregate_id).await
}
