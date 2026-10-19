//! The subscription that keeps `aliexpress_products` current for one tenant,
//! and the queries over it.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::{FromRow, SqlitePool};

use crate::aggregator::AliexpressProduct;
use crate::command::{LinkState, link_projection};

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-aliexpress/read-models";

#[derive(Debug, Clone)]
struct TenantId(String);

/// Builds the subscription of one tenant over every routing key. `pool` is
/// the shared read-model database and must be allowed to write.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    tenant_id: &str,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(TenantId(tenant_id.to_owned()))
        .handler(on_link_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_link_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<AliexpressProduct>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(link) = link_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_link(&pool, &tenant.0, &link).await
}

/// Writes the row of a link as the write model sees it.
pub async fn upsert_link(
    pool: &SqlitePool,
    tenant_id: &str,
    link: &LinkState,
) -> anyhow::Result<()> {
    sqlx::query(
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
    )
    .bind(tenant_id)
    .bind(&link.id)
    .bind(&link.ae_product_id)
    .bind(&link.product_id)
    .bind(link.status.as_str())
    .bind(link.offline)
    .bind(link.pricing.is_some())
    .bind(link.last_synced_at.map(|at| at as i64))
    .bind(&link.last_error)
    .bind(link.requested_at as i64)
    .bind(link.updated_at as i64)
    .bind(link.offered().count() as i64)
    .execute(pool)
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
pub async fn links_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    limit: i64,
) -> sqlx::Result<Vec<LinkRow>> {
    sqlx::query_as(concat!(
        columns!(),
        "WHERE tenant_id = ?1 ORDER BY requested_at DESC, id DESC LIMIT ?2"
    ))
    .bind(tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// The import that filled a catalog product, if one did.
pub async fn link_of_product(
    pool: &SqlitePool,
    tenant_id: &str,
    product_id: &str,
) -> sqlx::Result<Option<LinkRow>> {
    sqlx::query_as(concat!(
        columns!(),
        "WHERE tenant_id = ?1 AND product_id = ?2 AND status = 'linked'
         ORDER BY updated_at DESC LIMIT 1"
    ))
    .bind(tenant_id)
    .bind(product_id)
    .fetch_optional(pool)
    .await
}

/// The linked items of every shop last read before `before` (unix seconds),
/// oldest first, grouped by tenant: what a sweep reads again.
pub async fn links_due(pool: &SqlitePool, before: i64, limit: i64) -> sqlx::Result<Vec<LinkRow>> {
    sqlx::query_as(concat!(
        columns!(),
        "WHERE status = 'linked' AND (last_synced_at IS NULL OR last_synced_at < ?1)
         ORDER BY tenant_id, last_synced_at LIMIT ?2"
    ))
    .bind(before)
    .bind(limit)
    .fetch_all(pool)
    .await
}
