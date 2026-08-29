//! The `region_list` and `region_countries` read models.
//!
//! Both are configuration lookups, not money: the storefront resolves the
//! browser's region and `RegionVat` resolves a country's rate here. They are
//! eventually consistent, which for admin-edited config is invisible in
//! practice — and every amount an order snapshots still comes from the
//! authoritative checkout-time assessment.

use evento::metadata::Event;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::Executor;

use crate::aggregate::{RegionCountry, RegionCreated, RegionUpdated};
use crate::state::RegionState;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const READ_MODELS_SUBSCRIPTION: &str = "region-read-models";

/// One row of `region_list`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RegionRow {
    pub id: String,
    pub name: String,
    /// ISO currency code, parsed back with `Currency::from_code` for use.
    pub currency: String,
}

/// Every region, alphabetically — the picker and the admin list.
pub async fn list_regions(read_pool: &SqlitePool) -> anyhow::Result<Vec<RegionRow>> {
    let rows = sqlx::query_as::<_, RegionRow>(
        "SELECT id, name, currency FROM region_list ORDER BY name, id",
    )
    .fetch_all(read_pool)
    .await?;

    Ok(rows)
}

/// The region a country belongs to, with the country's rate.
/// `None` when no region claims the country.
pub async fn region_for_country(
    read_pool: &SqlitePool,
    country_code: &str,
) -> anyhow::Result<Option<(RegionRow, u32)>> {
    let row: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT r.id, r.name, r.currency, c.tax_rate_bps
         FROM region_countries c
         JOIN region_list r ON r.id = c.region_id
         WHERE c.country_code = ?",
    )
    .bind(country_code.trim().to_uppercase())
    .fetch_optional(read_pool)
    .await?;

    Ok(row.map(|(id, name, currency, rate)| {
        (
            RegionRow { id, name, currency },
            u32::try_from(rate).unwrap_or(0),
        )
    }))
}

/// The admin read-model subscription, unstarted.
///
/// Exposed so tests can drive it deterministically with
/// `.no_retry().run_once(&executor)` instead of racing a background task.
pub fn read_models_subscription(write_pool: SqlitePool) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(READ_MODELS_SUBSCRIPTION)
        .data(write_pool)
        .handler(on_region_created())
        .handler(on_region_updated())
        .strict()
}

/// Spawn the read-model subscription. The caller keeps the handle and calls
/// `shutdown()` on it.
pub async fn start_subscriptions(state: &RegionState) -> anyhow::Result<Vec<Subscription>> {
    let read_models = read_models_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    tracing::info!("region subscriptions started");
    Ok(vec![read_models])
}

fn write_pool<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>().ok_or_else(|| {
        anyhow::anyhow!(
            "`{READ_MODELS_SUBSCRIPTION}` subscription was started without a write pool"
        )
    })
}

/// Rewrite one region's country claims: drop its old rows, insert the new
/// list. `INSERT OR REPLACE` means a country claimed by two regions belongs to
/// the last writer — deterministic, and surfaced in the admin UI rather than
/// guarded here.
async fn replace_countries<E: evento::Executor>(
    ctx: &Context<'_, E>,
    region_id: &str,
    countries: &[RegionCountry],
) -> anyhow::Result<()> {
    let pool = write_pool(ctx)?;
    sqlx::query("DELETE FROM region_countries WHERE region_id = ?")
        .bind(region_id)
        .execute(&pool)
        .await?;

    for country in countries {
        sqlx::query(
            "INSERT OR REPLACE INTO region_countries (country_code, region_id, tax_rate_bps)
             VALUES (?, ?, ?)",
        )
        .bind(&country.code)
        .bind(region_id)
        .bind(i64::from(country.tax_rate_bps))
        .execute(&pool)
        .await?;
    }

    Ok(())
}

#[evento::subscription]
async fn on_region_created<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<RegionCreated>,
) -> anyhow::Result<()> {
    let created_at = i64::try_from(event.timestamp)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));

    sqlx::query(
        "INSERT INTO region_list (id, name, currency, created_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.name)
    .bind(event.data.currency.code())
    .bind(created_at)
    .execute(&write_pool(ctx)?)
    .await?;

    replace_countries(ctx, &event.aggregate_id, &event.data.countries).await
}

#[evento::subscription]
async fn on_region_updated<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<RegionUpdated>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE region_list SET name = ? WHERE id = ?")
        .bind(&event.data.name)
        .bind(&event.aggregate_id)
        .execute(&write_pool(ctx)?)
        .await?;

    replace_countries(ctx, &event.aggregate_id, &event.data.countries).await
}
