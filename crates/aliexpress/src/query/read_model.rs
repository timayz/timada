//! The subscription that keeps `aliexpress_products` and
//! `aliexpress_forwards` current for every tenant, and the queries over
//! them. One subscription follows every tenant's streams (its cursor lives
//! in the store); each row carries the tenant of the event that made it.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, FromRow};

use crate::aggregator::AliexpressProduct;
use crate::aggregator::forward::Forward;
use crate::command::{ForwardState, LinkState, forward_projection, link_projection};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-aliexpress/read-models";

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_link_event())
        .handler(on_forward_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_link_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<AliexpressProduct>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(link) = link_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_link(&db, &event.tenant, &link).await
}

#[evento::subscription_all]
async fn on_forward_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Forward>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(forward) = forward_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_forward(&db, &event.tenant, &forward).await
}

/// Writes the row of a forward as the write model sees it.
pub async fn upsert_forward(
    db: &Db,
    tenant_id: &str,
    forward: &ForwardState,
) -> anyhow::Result<()> {
    let ae_order_ids = forward
        .ae_orders
        .iter()
        .map(|order| order.id.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    db.execute(
        "INSERT INTO aliexpress_forwards (tenant_id, id, order_id, status, ae_order_ids,
            tracking, last_error, placed_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, ae_order_ids = excluded.ae_order_ids,
            tracking = excluded.tracking, last_error = excluded.last_error,
            placed_at = excluded.placed_at, updated_at = excluded.updated_at",
        (
            tenant_id,
            forward.id.as_str(),
            forward.order_id.as_str(),
            forward.status.as_str(),
            ae_order_ids,
            forward.tracking().join(" "),
            forward.last_error.as_deref(),
            forward.placed_at.map(|at| at as i64),
            forward.updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}

/// One row of `aliexpress_forwards`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ForwardRow {
    pub tenant_id: String,
    pub id: String,
    pub order_id: String,
    /// `requested`, `started`, `placed`, `failed` or `done`.
    pub status: String,
    /// Space-separated.
    pub ae_order_ids: String,
    /// Space-separated parcel numbers.
    pub tracking: String,
    pub last_error: Option<String>,
    pub placed_at: Option<i64>,
    pub updated_at: i64,
}

/// The orders of every shop placed on AliExpress and not settled yet, the
/// longest unchanged first, grouped by tenant: what a sweep reads again.
pub async fn forwards_due(db: &Db, limit: i64) -> anyhow::Result<Vec<ForwardRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, order_id, status, ae_order_ids, tracking, last_error,
            placed_at, updated_at
         FROM aliexpress_forwards
         WHERE status = 'placed'
         ORDER BY tenant_id, updated_at LIMIT ?1",
        (limit,),
    )
    .await
}

/// Writes the row of a link as the write model sees it.
pub async fn upsert_link(db: &Db, tenant_id: &str, link: &LinkState) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO aliexpress_products (tenant_id, id, ae_product_id, product_id, status,
            offline, own_pricing, last_synced_at, last_error, requested_at, updated_at,
            new_combinations)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            product_id = excluded.product_id, status = excluded.status,
            offline = excluded.offline, own_pricing = excluded.own_pricing,
            last_synced_at = excluded.last_synced_at, last_error = excluded.last_error,
            requested_at = excluded.requested_at, updated_at = excluded.updated_at,
            new_combinations = excluded.new_combinations",
        (
            tenant_id,
            link.id.as_str(),
            link.ae_product_id.as_str(),
            link.product_id.as_deref(),
            link.status.as_str(),
            link.offline,
            link.pricing.is_some(),
            link.last_synced_at.map(|at| at as i64),
            link.last_error.as_deref(),
            link.requested_at as i64,
            link.updated_at as i64,
            link.offered().count() as i64,
        ),
    )
    .await?;
    Ok(())
}

/// One row of `aliexpress_products`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct LinkRow {
    pub tenant_id: String,
    pub id: String,
    pub ae_product_id: String,
    pub product_id: Option<String>,
    /// `importing`, `linked`, `failed` or `unlinked`.
    pub status: String,
    pub offline: bool,
    /// Whether the product has its own price rule.
    pub own_pricing: bool,
    /// Unix seconds.
    pub last_synced_at: Option<i64>,
    pub last_error: Option<String>,
    pub requested_at: i64,
    pub updated_at: i64,
    /// Combinations a read found that the shop could add (ignored aside).
    pub new_combinations: i64,
}

/// The columns of a [`LinkRow`], shared by the queries below.
macro_rules! columns {
    () => {
        "SELECT tenant_id, id, ae_product_id, product_id, status, offline, own_pricing,
            last_synced_at, last_error, requested_at, updated_at, new_combinations
            FROM aliexpress_products "
    };
}

/// A shop's imports, most recently asked for first.
pub async fn links_of_tenant(db: &Db, tenant_id: &str, limit: i64) -> anyhow::Result<Vec<LinkRow>> {
    db.fetch_all(
        concat!(
            columns!(),
            "WHERE tenant_id = ?1 ORDER BY requested_at DESC, id DESC LIMIT ?2"
        ),
        (tenant_id, limit),
    )
    .await
}

/// The import that filled a catalog product, if one did.
pub async fn link_of_product(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
) -> anyhow::Result<Option<LinkRow>> {
    db.fetch_optional(
        concat!(
            columns!(),
            "WHERE tenant_id = ?1 AND product_id = ?2 AND status = 'linked'
         ORDER BY updated_at DESC LIMIT 1"
        ),
        (tenant_id, product_id),
    )
    .await
}

/// The linked items of every shop last read before `before` (unix seconds),
/// oldest first, grouped by tenant: what a sweep reads again.
pub async fn links_due(db: &Db, before: i64, limit: i64) -> anyhow::Result<Vec<LinkRow>> {
    db.fetch_all(
        concat!(
            columns!(),
            "WHERE status = 'linked' AND (last_synced_at IS NULL OR last_synced_at < ?1)
         ORDER BY tenant_id, last_synced_at LIMIT ?2"
        ),
        (before, limit),
    )
    .await
}
