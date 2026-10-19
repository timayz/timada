//! Which shops have an AliExpress account connected, and until when its
//! access lasts: what the host's renewal goes by, whether or not the shop has
//! anything imported to read again.
//!
//! One row per connected shop, rewritten from the strict write model, so a
//! repeated delivery changes nothing. The subscription has its own cursor:
//! started on an existing log, it finds every shop connected before it.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::{AliexpressAuthorized, AliexpressConfigured, AliexpressRemoved};
use crate::command::tenant_projection;

/// The cursor identity of the AliExpress access subscription. One per node.
pub const ALIEXPRESS_ACCESS_SUBSCRIPTION: &str = "timada-tenancy/aliexpress-access";

/// Builds the subscription over every routing key. `pool` must be allowed to
/// write; start it with `.start(&executor)` and keep the handle for shutdown.
pub fn aliexpress_access_subscription<E: Executor>(pool: SqlitePool) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ALIEXPRESS_ACCESS_SUBSCRIPTION)
        .data(pool)
        .handler(on_aliexpress_configured())
        .handler(on_aliexpress_authorized())
        .handler(on_aliexpress_removed())
        .any_routing_key()
}

/// The shops whose access lapses at or before `before` (Unix seconds) and
/// can still be renewed at `now` (the refresh token has not lapsed), the
/// soonest first, at most `limit`.
pub async fn aliexpress_access_due(
    pool: &SqlitePool,
    before: i64,
    now: i64,
    limit: i64,
) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT tenant_id FROM tenancy_aliexpress_access
         WHERE expires_at <= ?1 AND (refresh_expires_at = 0 OR refresh_expires_at > ?2)
         ORDER BY expires_at, tenant_id
         LIMIT ?3",
    )
    .bind(before)
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Rewrites the shop's row from its state: gone once no account is
/// connected (removed, or another app set up).
async fn rewrite<E: Executor>(context: &Context<'_, E>, tenant_id: &str) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant = tenant_projection()
        .load(tenant_id)
        .execute(context.executor)
        .await?;
    let authorization = tenant
        .as_ref()
        .and_then(|tenant| tenant.aliexpress.as_ref())
        .and_then(|settings| settings.authorization.as_ref());
    match authorization {
        Some(authorization) => {
            sqlx::query(
                "INSERT INTO tenancy_aliexpress_access (tenant_id, expires_at, refresh_expires_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (tenant_id) DO UPDATE SET
                    expires_at = excluded.expires_at,
                    refresh_expires_at = excluded.refresh_expires_at",
            )
            .bind(tenant_id)
            .bind(authorization.expires_at as i64)
            .bind(authorization.refresh_expires_at as i64)
            .execute(&pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM tenancy_aliexpress_access WHERE tenant_id = ?1")
                .bind(tenant_id)
                .execute(&pool)
                .await?;
        }
    }
    Ok(())
}

#[evento::subscription]
async fn on_aliexpress_configured<E: Executor>(
    context: &Context<'_, E>,
    event: Event<AliexpressConfigured>,
) -> anyhow::Result<()> {
    rewrite(context, &event.aggregate_id).await
}

#[evento::subscription]
async fn on_aliexpress_authorized<E: Executor>(
    context: &Context<'_, E>,
    event: Event<AliexpressAuthorized>,
) -> anyhow::Result<()> {
    rewrite(context, &event.aggregate_id).await
}

#[evento::subscription]
async fn on_aliexpress_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<AliexpressRemoved>,
) -> anyhow::Result<()> {
    rewrite(context, &event.aggregate_id).await
}
