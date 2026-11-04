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
use crate::value_object::{Share, shard_of};

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

/// The orders of every shop placed on AliExpress and not settled yet: what a
/// sweep reads again. The shops take turns, each its longest unchanged first,
/// so one shop's open orders never crowd another shop out. Grouped by tenant,
/// as the sweep paces one shop's reads.
pub async fn forwards_due(db: &Db, limit: i64) -> anyhow::Result<Vec<ForwardRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, order_id, status, ae_order_ids, tracking, last_error,
            placed_at, updated_at
         FROM aliexpress_forwards
         WHERE (tenant_id, id) IN (
            SELECT tenant_id, id FROM (
                SELECT tenant_id, id, updated_at, ROW_NUMBER() OVER (
                    PARTITION BY tenant_id ORDER BY updated_at, id
                ) AS turn
                FROM aliexpress_forwards
                WHERE status = 'placed'
            ) ORDER BY turn, updated_at, tenant_id LIMIT ?1
         )
         ORDER BY tenant_id, updated_at, id",
        (limit,),
    )
    .await
}

/// The forward that bought `ae_order_id` on AliExpress, whichever tenant's,
/// as the read model has it (a push may arrive before the row does).
pub async fn forward_of_ae_order(db: &Db, ae_order_id: &str) -> anyhow::Result<Option<ForwardRow>> {
    db.fetch_optional(
        "SELECT tenant_id, id, order_id, status, ae_order_ids, tracking, last_error,
            placed_at, updated_at
         FROM aliexpress_forwards
         WHERE ' ' || ae_order_ids || ' ' LIKE '% ' || ?1 || ' %'
         ORDER BY updated_at DESC LIMIT 1",
        (ae_order_id,),
    )
    .await
}

/// Writes the row of a link as the write model sees it. The times of the reads
/// only move forward: a read that changed nothing records no event, so this
/// node may know of a later one than the link does ([`note_read`]).
pub async fn upsert_link(db: &Db, tenant_id: &str, link: &LinkState) -> anyhow::Result<()> {
    let tried = link.last_requested_at.max(link.last_synced_at);
    db.execute(
        "INSERT INTO aliexpress_products (tenant_id, id, ae_product_id, product_id, status,
            offline, own_pricing, last_synced_at, last_error, requested_at, updated_at,
            new_combinations, last_tried_at, shard)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            product_id = excluded.product_id, status = excluded.status,
            offline = excluded.offline, own_pricing = excluded.own_pricing,
            last_synced_at = NULLIF(MAX(COALESCE(last_synced_at, 0),
                COALESCE(excluded.last_synced_at, 0)), 0),
            last_error = excluded.last_error,
            requested_at = excluded.requested_at, updated_at = excluded.updated_at,
            new_combinations = excluded.new_combinations,
            last_tried_at = NULLIF(MAX(COALESCE(last_tried_at, 0),
                COALESCE(excluded.last_tried_at, 0)), 0)",
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
            tried.map(|at| at as i64),
            i64::from(shard_of(&link.id)),
        ),
    )
    .await?;
    Ok(())
}

/// A read of the link worked at `at` (unix seconds), whether or not it found
/// anything to record.
pub async fn note_read(db: &Db, tenant_id: &str, link_id: &str, at: u64) -> anyhow::Result<()> {
    db.execute(
        "UPDATE aliexpress_products SET
            last_synced_at = MAX(COALESCE(last_synced_at, 0), ?3),
            last_tried_at = MAX(COALESCE(last_tried_at, 0), ?3)
         WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, link_id, at as i64),
    )
    .await?;
    Ok(())
}

/// A read of the link was asked for or made at `at` (unix seconds), which is
/// what makes it due again a period later: one that failed, or a shop that
/// cannot read for now, takes no turn away from the others meanwhile.
pub async fn note_tried(db: &Db, tenant_id: &str, link_id: &str, at: u64) -> anyhow::Result<()> {
    db.execute(
        "UPDATE aliexpress_products SET last_tried_at = MAX(COALESCE(last_tried_at, 0), ?3)
         WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, link_id, at as i64),
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
    /// Unix seconds of the last read that worked, as far as this node knows.
    pub last_synced_at: Option<i64>,
    pub last_error: Option<String>,
    pub requested_at: i64,
    pub updated_at: i64,
    /// Combinations a read found that the shop could add (ignored aside).
    pub new_combinations: i64,
    /// Unix seconds of the last read asked for or made, as far as this node
    /// knows, whether or not it worked.
    pub last_tried_at: Option<i64>,
}

/// The columns of a [`LinkRow`], shared by the queries below.
macro_rules! columns {
    () => {
        "SELECT tenant_id, id, ae_product_id, product_id, status, offline, own_pricing,
            last_synced_at, last_error, requested_at, updated_at, new_combinations,
            last_tried_at
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

/// The linked items of every shop that `share` holds, last tried before
/// `before` (unix seconds): what a sweep asks to read again. The shops take
/// turns, each its oldest first, so a backlog never crowds another shop out.
/// Grouped by tenant, as the sweep paces one shop's requests.
pub async fn links_due(
    db: &Db,
    before: i64,
    share: Share,
    limit: i64,
) -> anyhow::Result<Vec<LinkRow>> {
    db.fetch_all(
        concat!(
            columns!(),
            "WHERE (tenant_id, id) IN (
                SELECT tenant_id, id FROM (
                    SELECT tenant_id, id, last_tried_at, ROW_NUMBER() OVER (
                        PARTITION BY tenant_id ORDER BY last_tried_at, id
                    ) AS turn
                    FROM aliexpress_products
                    WHERE status = 'linked'
                        AND (last_tried_at IS NULL OR last_tried_at < ?1)
                        AND shard % ?3 = ?4
                ) ORDER BY turn, last_tried_at, tenant_id LIMIT ?2
            )
            ORDER BY tenant_id, last_tried_at, id"
        ),
        (before, limit, share.of as i64, share.index as i64),
    )
    .await
}
