//! Which companies have an AliExpress account connected, and until when its
//! access lasts: what the host's renewal goes by, whether or not any of the
//! company's shops has anything imported to read again.
//!
//! One row per connected company, rewritten from the strict write model, so
//! a repeated delivery changes nothing. The subscription has its own cursor:
//! started on an existing log, it finds every company connected before it.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::company::{AliexpressAuthorized, AliexpressDisconnected};
use crate::command::company_projection;

/// The cursor identity of the AliExpress accounts subscription. One per node.
pub const ALIEXPRESS_ACCOUNTS_SUBSCRIPTION: &str = "timada-tenancy/aliexpress-accounts";

/// Builds the subscription over every routing key. `pool` must be allowed to
/// write; start it with `.start(&executor)` and keep the handle for shutdown.
pub fn aliexpress_accounts_subscription<E: Executor>(pool: SqlitePool) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ALIEXPRESS_ACCOUNTS_SUBSCRIPTION)
        .data(pool)
        .handler(on_aliexpress_authorized())
        .handler(on_aliexpress_disconnected())
        .any_routing_key()
}

/// The companies whose access lapses at or before `before` (Unix seconds)
/// and can still be renewed at `now` (the refresh token has not lapsed), the
/// soonest first, at most `limit`.
pub async fn aliexpress_accounts_due(
    pool: &SqlitePool,
    before: i64,
    now: i64,
    limit: i64,
) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT company_id FROM tenancy_aliexpress_accounts
         WHERE expires_at <= ?1 AND (refresh_expires_at = 0 OR refresh_expires_at > ?2)
         ORDER BY expires_at, company_id
         LIMIT ?3",
    )
    .bind(before)
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Rewrites the company's row from its state: gone once no account is
/// connected.
async fn rewrite<E: Executor>(context: &Context<'_, E>, company_id: &str) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let company = company_projection()
        .load(company_id)
        .execute(context.executor)
        .await?;
    match company
        .as_ref()
        .and_then(|company| company.aliexpress.as_ref())
    {
        Some(authorization) => {
            sqlx::query(
                "INSERT INTO tenancy_aliexpress_accounts (company_id, expires_at, refresh_expires_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (company_id) DO UPDATE SET
                    expires_at = excluded.expires_at,
                    refresh_expires_at = excluded.refresh_expires_at",
            )
            .bind(company_id)
            .bind(authorization.expires_at as i64)
            .bind(authorization.refresh_expires_at as i64)
            .execute(&pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM tenancy_aliexpress_accounts WHERE company_id = ?1")
                .bind(company_id)
                .execute(&pool)
                .await?;
        }
    }
    Ok(())
}

#[evento::subscription]
async fn on_aliexpress_authorized<E: Executor>(
    context: &Context<'_, E>,
    event: Event<AliexpressAuthorized>,
) -> anyhow::Result<()> {
    rewrite(context, &event.aggregate_id).await
}

#[evento::subscription]
async fn on_aliexpress_disconnected<E: Executor>(
    context: &Context<'_, E>,
    event: Event<AliexpressDisconnected>,
) -> anyhow::Result<()> {
    rewrite(context, &event.aggregate_id).await
}
