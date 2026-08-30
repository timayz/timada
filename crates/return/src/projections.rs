//! The `admin_return_list` read model.

use evento::metadata::Event;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::Executor;

use crate::aggregate::{ReturnApproved, ReturnRefunded, ReturnRejected, ReturnRequested};
use crate::saga::return_flow_subscription;
use crate::state::ReturnState;
use crate::view::ReturnStatus;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const READ_MODELS_SUBSCRIPTION: &str = "return-read-models";

/// One row of the admin return list.
#[derive(Debug, sqlx::FromRow)]
pub struct AdminReturnRow {
    pub id: String,
    pub order_id: String,
    /// One of [`ReturnStatus::as_str`].
    pub status: String,
    pub reason: String,
    pub reject_reason: Option<String>,
    pub requested_at: i64,
}

impl AdminReturnRow {
    /// `YYYY-MM-DD HH:MM UTC`, so the list is readable at a glance.
    pub fn requested(&self) -> String {
        timada_core::format_utc_datetime(self.requested_at)
    }
}

/// Newest returns first, capped so the page stays cheap.
pub async fn recent_returns(
    read_pool: &SqlitePool,
    limit: i64,
) -> anyhow::Result<Vec<AdminReturnRow>> {
    let rows = sqlx::query_as::<_, AdminReturnRow>(
        "SELECT id, order_id, status, reason, reject_reason, requested_at
           FROM admin_return_list
          ORDER BY requested_at DESC, id DESC
          LIMIT ?",
    )
    .bind(limit)
    .fetch_all(read_pool)
    .await?;

    Ok(rows)
}

/// The read-model subscription, unstarted — tests drive it with
/// `.no_retry().run_once(&executor)`.
pub fn read_models_subscription(write_pool: SqlitePool) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(READ_MODELS_SUBSCRIPTION)
        .data(write_pool)
        .handler(on_requested())
        .handler(on_approved())
        .handler(on_rejected())
        .handler(on_refunded())
        .strict()
}

/// Spawn every background subscription this crate owns: the admin read model
/// and the return flow. The caller keeps the handles and calls `shutdown()`.
pub async fn start_subscriptions(state: &ReturnState) -> anyhow::Result<Vec<Subscription>> {
    let flow = return_flow_subscription(state.ctx.executor.clone(), state.provider.clone())
        .start(&state.ctx.executor)
        .await?;

    let read_models = read_models_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    tracing::info!("return subscriptions started");
    Ok(vec![flow, read_models])
}

fn write_pool<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>().ok_or_else(|| {
        anyhow::anyhow!(
            "`{READ_MODELS_SUBSCRIPTION}` subscription was started without a write pool"
        )
    })
}

async fn set_status<E: evento::Executor>(
    ctx: &Context<'_, E>,
    return_id: &str,
    status: ReturnStatus,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE admin_return_list SET status = ? WHERE id = ?")
        .bind(status.as_str())
        .bind(return_id)
        .execute(&write_pool(ctx)?)
        .await?;

    Ok(())
}

/// A re-request after a rejection refreshes the row wholesale — it is the
/// same conversation, reopened.
#[evento::subscription]
async fn on_requested<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnRequested>,
) -> anyhow::Result<()> {
    let requested_at = i64::try_from(event.timestamp)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));

    sqlx::query(
        "INSERT INTO admin_return_list (id, order_id, status, reason, requested_at)
         VALUES (?, ?, 'requested', ?, ?)
         ON CONFLICT (id) DO UPDATE
             SET status = 'requested', reason = excluded.reason,
                 requested_at = excluded.requested_at",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.order_id)
    .bind(&event.data.reason)
    .bind(requested_at)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_approved<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnApproved>,
) -> anyhow::Result<()> {
    set_status(ctx, &event.aggregate_id, ReturnStatus::Approved).await
}

#[evento::subscription]
async fn on_rejected<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnRejected>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE admin_return_list SET status = 'rejected', reject_reason = ? WHERE id = ?")
        .bind(&event.data.reason)
        .bind(&event.aggregate_id)
        .execute(&write_pool(ctx)?)
        .await?;

    Ok(())
}

#[evento::subscription]
async fn on_refunded<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnRefunded>,
) -> anyhow::Result<()> {
    set_status(ctx, &event.aggregate_id, ReturnStatus::Refunded).await
}
