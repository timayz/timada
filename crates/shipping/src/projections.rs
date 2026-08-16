//! The `admin_shipment_list` read model.
//!
//! One SQL table serving exactly one query shape: the admin shipping page's
//! list. It is eventually consistent — a handler runs after the event is
//! committed, so the redirect straight after a tracking refresh may briefly
//! show the previous status.

use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::Executor;

use crate::aggregate::{ShipmentCreated, ShipmentDelivered, ShipmentDispatched};
use crate::state::ShippingState;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const ADMIN_SUBSCRIPTION: &str = "shipping-admin";

/// One row of the admin shipment list.
#[derive(Debug, sqlx::FromRow)]
pub struct AdminShipmentRow {
    pub id: String,
    pub order_id: String,
    pub supplier_id: String,
    pub external_ref: String,
    pub tracking_number: Option<String>,
    pub carrier: Option<String>,
    /// `created` | `dispatched` | `delivered`.
    pub status: String,
}

impl AdminShipmentRow {
    /// A delivered parcel has nowhere left to go, so the admin page hides its
    /// refresh button.
    pub fn is_refreshable(&self) -> bool {
        self.status != "delivered"
    }
}

/// Newest shipments first, capped so the page stays cheap.
pub async fn recent_shipments(
    read_pool: &SqlitePool,
    limit: i64,
) -> anyhow::Result<Vec<AdminShipmentRow>> {
    let rows = sqlx::query_as::<_, AdminShipmentRow>(
        "SELECT id, order_id, supplier_id, external_ref, tracking_number, carrier, status \
         FROM admin_shipment_list \
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
async fn on_shipment_created<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: evento::metadata::Event<ShipmentCreated>,
) -> anyhow::Result<()> {
    // Epoch milliseconds; sub-second precision keeps the list ordered when
    // several shipments are created within the same second.
    let created_at = i64::try_from(event.timestamp)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));

    sqlx::query(
        "INSERT INTO admin_shipment_list \
             (id, order_id, supplier_id, external_ref, status, created_at) \
         VALUES (?, ?, ?, ?, 'created', ?) \
         ON CONFLICT(id) DO UPDATE SET order_id = excluded.order_id, \
             supplier_id = excluded.supplier_id, external_ref = excluded.external_ref, \
             created_at = excluded.created_at",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.order_id)
    .bind(&event.data.supplier_id)
    .bind(&event.data.external_ref)
    .bind(created_at)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_shipment_dispatched<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: evento::metadata::Event<ShipmentDispatched>,
) -> anyhow::Result<()> {
    // The row is always there: one subscription replays an aggregate's events
    // in version order, so `ShipmentCreated` was handled first.
    sqlx::query(
        "UPDATE admin_shipment_list \
         SET status = 'dispatched', tracking_number = ?, carrier = ? WHERE id = ?",
    )
    .bind(&event.data.tracking_number)
    .bind(&event.data.carrier)
    .bind(&event.aggregate_id)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_shipment_delivered<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: evento::metadata::Event<ShipmentDelivered>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE admin_shipment_list SET status = 'delivered' WHERE id = ?")
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
        .handler(on_shipment_created())
        .handler(on_shipment_dispatched())
        .handler(on_shipment_delivered())
        .strict()
}

/// Spawn every background subscription this crate owns.
///
/// The caller keeps the handles and calls `shutdown()` on them.
pub async fn start_subscriptions(state: &ShippingState) -> anyhow::Result<Vec<Subscription>> {
    let admin = admin_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    tracing::info!(
        subscription = ADMIN_SUBSCRIPTION,
        "shipping subscriptions started"
    );
    Ok(vec![admin])
}
