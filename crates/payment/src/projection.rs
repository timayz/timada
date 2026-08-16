//! The `admin_payment_list` read model.
//!
//! One row per payment, serving the admin payments list and nothing else.
//! Handlers are idempotent: the insert conflicts away and the status updates
//! are absolute, so a replayed chunk converges on the same row.

use evento::Executor;
use evento::context::Data;
use evento::metadata::Event;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;

use crate::aggregate::{ChargeCaptured, ChargeFailed, ChargeRefunded, ChargeRequested};
use crate::state::PaymentState;

/// Start the payment context's subscriptions. The caller owns the handles and
/// is responsible for shutting them down.
pub async fn start_subscriptions(state: &PaymentState) -> anyhow::Result<Vec<Subscription>> {
    let subscription = admin_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    Ok(vec![subscription])
}

/// The admin read-model subscription, unstarted — callers that need
/// deterministic draining (tests, the e2e slice) use `.no_retry().run_once()`.
pub fn admin_subscription(write_pool: SqlitePool) -> SubscriptionBuilder<timada_core::Executor> {
    SubscriptionBuilder::new("payment-admin")
        .data(Data::new(write_pool))
        .handler(on_charge_requested())
        .handler(on_charge_captured())
        .handler(on_charge_failed())
        .handler(on_charge_refunded())
}

/// Event timestamps are seconds + a millisecond remainder; the read model
/// stores them merged so the list can order by a single column.
fn millis(event: &evento::Event) -> anyhow::Result<i64> {
    let millis = event
        .timestamp
        .saturating_mul(1000)
        .saturating_add(u64::from(event.timestamp_subsec));
    Ok(i64::try_from(millis)?)
}

#[evento::subscription]
async fn on_charge_requested<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ChargeRequested>,
) -> anyhow::Result<()> {
    let db: Data<SqlitePool> = ctx.extract();
    sqlx::query(
        "INSERT INTO admin_payment_list
             (id, order_id, provider, amount_cents, currency, status, created_at)
         VALUES (?, ?, ?, ?, ?, 'requested', ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.order_id)
    .bind(&event.data.provider)
    .bind(event.data.amount.amount_cents)
    .bind(event.data.amount.currency.code())
    .bind(millis(&event)?)
    .execute(db.get_ref())
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_charge_captured<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ChargeCaptured>,
) -> anyhow::Result<()> {
    let db: Data<SqlitePool> = ctx.extract();
    sqlx::query(
        "UPDATE admin_payment_list
            SET status = 'captured', provider_charge_ref = ?
          WHERE id = ?",
    )
    .bind(&event.data.provider_charge_ref)
    .bind(&event.aggregate_id)
    .execute(db.get_ref())
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_charge_failed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ChargeFailed>,
) -> anyhow::Result<()> {
    let db: Data<SqlitePool> = ctx.extract();
    sqlx::query("UPDATE admin_payment_list SET status = 'failed', reason = ? WHERE id = ?")
        .bind(&event.data.reason)
        .bind(&event.aggregate_id)
        .execute(db.get_ref())
        .await?;

    Ok(())
}

#[evento::subscription]
async fn on_charge_refunded<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ChargeRefunded>,
) -> anyhow::Result<()> {
    let db: Data<SqlitePool> = ctx.extract();
    sqlx::query("UPDATE admin_payment_list SET status = 'refunded' WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(db.get_ref())
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::PaymentProvider;
    use std::sync::Arc;
    use timada_core::{Currency, Money};

    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        status: String,
        amount_cents: i64,
        provider_charge_ref: Option<String>,
        reason: Option<String>,
    }

    #[tokio::test]
    async fn projects_the_lifecycle_of_a_payment_into_one_row() {
        let db = crate::test_support::temp_db().await;
        let provider: Arc<dyn PaymentProvider> = Arc::new(crate::FakePaymentProvider);

        let captured = crate::request_charge(
            &db.ctx.executor,
            &provider,
            "order-ok",
            Money::new(1250, Currency::Eur),
        )
        .await
        .unwrap();
        let declined = crate::request_charge(
            &db.ctx.executor,
            &provider,
            "order-ko",
            Money::new(1999, Currency::Eur),
        )
        .await
        .unwrap();

        let mut subscription = admin_subscription(db.ctx.write_pool.clone()).no_retry();
        subscription.run_once(&db.ctx.executor).await.unwrap();

        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, status, amount_cents, provider_charge_ref, reason
               FROM admin_payment_list ORDER BY id",
        )
        .fetch_all(&db.ctx.read_pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);

        let ok = rows.iter().find(|r| r.id == captured).unwrap();
        assert_eq!(ok.status, "captured");
        assert_eq!(ok.amount_cents, 1250);
        assert!(
            ok.provider_charge_ref
                .as_deref()
                .unwrap_or_default()
                .starts_with("FAKE-")
        );

        let ko = rows.iter().find(|r| r.id == declined).unwrap();
        assert_eq!(ko.status, "failed");
        assert!(
            ko.reason
                .as_deref()
                .unwrap_or_default()
                .contains("declined")
        );

        // Refunding advances the same row rather than adding another.
        crate::refund(&db.ctx.executor, &provider, &captured)
            .await
            .unwrap();
        subscription.run_once(&db.ctx.executor).await.unwrap();

        let (count, status): (i64, String) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM admin_payment_list), status
               FROM admin_payment_list WHERE id = ?",
        )
        .bind(&captured)
        .fetch_one(&db.ctx.read_pool)
        .await
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(status, "refunded");
    }
}
