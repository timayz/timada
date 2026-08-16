//! The `admin_supplier_order_list` read model.
//!
//! One SQL table serving exactly one query shape: the admin suppliers page's
//! "recent supplier orders" list. It is eventually consistent — a handler runs
//! after the event is committed, so a redirect straight after `forward_order`
//! may briefly not see the row.

use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::Executor;

use crate::aggregate::{SupplierOrderConfirmed, SupplierOrderPlaced, SupplierOrderRejected};
use crate::state::DropshipState;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const ADMIN_SUBSCRIPTION: &str = "dropship-admin";

/// One row of the admin supplier-order list.
#[derive(Debug, sqlx::FromRow)]
pub struct AdminSupplierOrderRow {
    pub id: String,
    pub order_id: String,
    pub supplier_id: String,
    pub external_ref: Option<String>,
    /// `placed` | `confirmed` | `rejected`.
    pub status: String,
    pub reason: Option<String>,
}

/// Newest supplier orders first, capped so the page stays cheap.
pub async fn recent_supplier_orders(
    read_pool: &SqlitePool,
    limit: i64,
) -> anyhow::Result<Vec<AdminSupplierOrderRow>> {
    let rows = sqlx::query_as::<_, AdminSupplierOrderRow>(
        "SELECT id, order_id, supplier_id, external_ref, status, reason \
         FROM admin_supplier_order_list \
         ORDER BY created_at DESC, id DESC \
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(read_pool)
    .await?;

    Ok(rows)
}

fn write_pool<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>().ok_or_else(|| {
        anyhow::anyhow!("`{ADMIN_SUBSCRIPTION}` subscription was started without a write pool")
    })
}

#[evento::subscription]
async fn on_supplier_order_placed<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: evento::metadata::Event<SupplierOrderPlaced>,
) -> anyhow::Result<()> {
    // Epoch milliseconds; sub-second precision keeps the list ordered when
    // several supplier orders are forwarded within the same second.
    let created_at = i64::try_from(event.timestamp)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));

    sqlx::query(
        "INSERT INTO admin_supplier_order_list (id, order_id, supplier_id, status, created_at) \
         VALUES (?, ?, ?, 'placed', ?) \
         ON CONFLICT(id) DO UPDATE SET order_id = excluded.order_id, \
             supplier_id = excluded.supplier_id, created_at = excluded.created_at",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.order_id)
    .bind(&event.data.supplier_id)
    .bind(created_at)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_supplier_order_confirmed<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: evento::metadata::Event<SupplierOrderConfirmed>,
) -> anyhow::Result<()> {
    // The row is always there: one subscription replays an aggregate's events
    // in version order, so `SupplierOrderPlaced` was handled first.
    sqlx::query(
        "UPDATE admin_supplier_order_list \
         SET status = 'confirmed', external_ref = ?, reason = NULL WHERE id = ?",
    )
    .bind(&event.data.external_ref)
    .bind(&event.aggregate_id)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_supplier_order_rejected<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: evento::metadata::Event<SupplierOrderRejected>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE admin_supplier_order_list SET status = 'rejected', reason = ? WHERE id = ?",
    )
    .bind(&event.data.reason)
    .bind(&event.aggregate_id)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

/// The admin read-model subscription, unstarted.
///
/// Exposed so tests and the demo app's end-to-end run can drive it
/// deterministically with `.no_retry().run_once(&executor)` instead of racing a
/// background task.
pub fn admin_subscription(write_pool: SqlitePool) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(ADMIN_SUBSCRIPTION)
        .data(write_pool)
        .handler(on_supplier_order_placed())
        .handler(on_supplier_order_confirmed())
        .handler(on_supplier_order_rejected())
        .strict()
}

/// Spawn every background subscription this crate owns.
///
/// The caller keeps the handles and calls `shutdown()` on them.
pub async fn start_subscriptions(state: &DropshipState) -> anyhow::Result<Vec<Subscription>> {
    let admin = admin_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    tracing::info!(
        subscription = ADMIN_SUBSCRIPTION,
        "dropship subscriptions started"
    );
    Ok(vec![admin])
}
