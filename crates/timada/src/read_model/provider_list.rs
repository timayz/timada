//! Read model for the admin providers page: one row per provider connection.
//!
//! Deliberately excludes credentials — those never leave the write side.

use anyhow::Result;
use evento::Executor;
use evento::metadata::Event;
use evento::sql::RwSqlite;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use sqlx_migrator::{sqlite_migration, vec_box};

use crate::provider_connection::{
    ProviderConnected, ProviderCredentialsConfigured, ProviderDisabled, ProviderEnabled,
};

pub struct M0001CreateProviderList;

sqlite_migration!(
    M0001CreateProviderList,
    "timada",
    "m0001_create_provider_list",
    vec_box![],
    vec_box![(
        "CREATE TABLE provider_list (
            id TEXT PRIMARY KEY,
            provider_kind TEXT NOT NULL,
            display_name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 0,
            credentials_configured INTEGER NOT NULL DEFAULT 0
        )",
        "DROP TABLE provider_list"
    )]
);

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProviderListRow {
    pub id: String,
    pub provider_kind: String,
    pub display_name: String,
    pub enabled: bool,
    pub credentials_configured: bool,
}

/// Every connection, newest first (ids are ULIDs, so they sort by time).
pub async fn all(db: &SqlitePool) -> Result<Vec<ProviderListRow>> {
    let rows = sqlx::query_as(
        "SELECT id, provider_kind, display_name, enabled, credentials_configured
         FROM provider_list ORDER BY id DESC",
    )
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn by_id(db: &SqlitePool, id: &str) -> Result<Option<ProviderListRow>> {
    let row = sqlx::query_as(
        "SELECT id, provider_kind, display_name, enabled, credentials_configured
         FROM provider_list WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

fn db<E: Executor>(ctx: &Context<'_, E>) -> Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool not injected into subscription"))
}

#[evento::subscription]
async fn on_provider_connected<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProviderConnected>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO provider_list (id, provider_kind, display_name)
         VALUES (?, ?, ?) ON CONFLICT(id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.provider_kind)
    .bind(&event.data.display_name)
    .execute(&db(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn on_provider_credentials_configured<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProviderCredentialsConfigured>,
) -> Result<()> {
    sqlx::query("UPDATE provider_list SET credentials_configured = ? WHERE id = ?")
        .bind(!event.data.config.is_empty())
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn on_provider_enabled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProviderEnabled>,
) -> Result<()> {
    sqlx::query("UPDATE provider_list SET enabled = 1 WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn on_provider_disabled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProviderDisabled>,
) -> Result<()> {
    sqlx::query("UPDATE provider_list SET enabled = 0 WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

/// Start the subscription that keeps `provider_list` up to date.
///
/// `write_pool` is the single-connection write pool: read-model upserts are
/// serialized through the same connection as event-store commits.
pub(crate) async fn start(executor: &RwSqlite, write_pool: SqlitePool) -> Result<Subscription> {
    SubscriptionBuilder::<RwSqlite>::new("provider-list")
        .handler(on_provider_connected())
        .handler(on_provider_credentials_configured())
        .handler(on_provider_enabled())
        .handler(on_provider_disabled())
        .data(write_pool)
        .all()
        .start(executor)
        .await
}
