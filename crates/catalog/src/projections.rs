//! The catalog's three SQL read models.
//!
//! `store_product_detail` and `admin_product_list` are written on import;
//! `store_product_list` — the storefront grid — is populated only when a
//! product is published and emptied again when it is archived, so the grid
//! query needs no status filter at all.
//!
//! All three are maintained by one subscription. They are eventually
//! consistent: an admin redirected straight after publishing may briefly not
//! see the change. Anything needing read-your-own-write goes through
//! [`load_product`](crate::load_product) instead.

use evento::metadata::Event;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::Executor;

use crate::aggregate::{ProductArchived, ProductImported, ProductPriceSet, ProductPublished};
use crate::state::CatalogState;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const READ_MODELS_SUBSCRIPTION: &str = "catalog-read-models";

fn write_pool<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>().ok_or_else(|| {
        anyhow::anyhow!(
            "`{READ_MODELS_SUBSCRIPTION}` subscription was started without a write pool"
        )
    })
}

/// Event timestamps are seconds plus a millisecond remainder; the read models
/// store them merged so a list can order by a single column.
fn millis<T>(event: &Event<T>) -> anyhow::Result<i64> {
    let millis = event
        .timestamp
        .saturating_mul(1000)
        .saturating_add(u64::from(event.timestamp_subsec));
    Ok(i64::try_from(millis)?)
}

#[evento::subscription]
async fn on_product_imported<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductImported>,
) -> anyhow::Result<()> {
    let pool = write_pool(ctx)?;
    let created_at = millis(&event)?;
    let currency = event.data.price.currency.code();

    // `DO NOTHING` rather than an update: `ProductImported` happens once per
    // product, so a conflict only means the subscription replayed a chunk —
    // and re-inserting would clobber `published` / `status` set by later
    // events in the same replay.
    sqlx::query(
        "INSERT INTO store_product_detail
             (id, title, description, price_cents, currency, image_url,
              supplier_id, supplier_product_ref, published, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.title)
    .bind(&event.data.description)
    .bind(event.data.price.amount_cents)
    .bind(currency)
    .bind(&event.data.image_url)
    .bind(&event.data.supplier_id)
    .bind(&event.data.supplier_product_ref)
    .bind(created_at)
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO admin_product_list
             (id, title, price_cents, currency, supplier_id, supplier_product_ref,
              status, created_at)
         VALUES (?, ?, ?, ?, ?, ?, 'draft', ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.title)
    .bind(event.data.price.amount_cents)
    .bind(currency)
    .bind(&event.data.supplier_id)
    .bind(&event.data.supplier_product_ref)
    .bind(created_at)
    .execute(&pool)
    .await?;

    // The base price answers for its own currency until an explicit
    // `ProductPriceSet` overrides it.
    sqlx::query(
        "INSERT OR REPLACE INTO store_product_prices (product_id, currency, amount_cents)
         VALUES (?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(currency)
    .bind(event.data.price.amount_cents)
    .execute(&pool)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_product_price_set<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductPriceSet>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO store_product_prices (product_id, currency, amount_cents)
         VALUES (?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(event.data.price.currency.code())
    .bind(event.data.price.amount_cents)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_product_published<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductPublished>,
) -> anyhow::Result<()> {
    let pool = write_pool(ctx)?;

    // The detail row is always there: one subscription replays an aggregate's
    // events in version order, so `ProductImported` was handled first. Copying
    // from it keeps the grid row and the detail page on the same snapshot
    // without re-deriving anything from the event.
    sqlx::query(
        "INSERT OR REPLACE INTO store_product_list
             (id, title, price_cents, currency, image_url, created_at)
         SELECT id, title, price_cents, currency, image_url, created_at
           FROM store_product_detail
          WHERE id = ?",
    )
    .bind(&event.aggregate_id)
    .execute(&pool)
    .await?;

    sqlx::query("UPDATE store_product_detail SET published = 1 WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool)
        .await?;

    sqlx::query("UPDATE admin_product_list SET status = 'published' WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool)
        .await?;

    Ok(())
}

#[evento::subscription]
async fn on_product_archived<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductArchived>,
) -> anyhow::Result<()> {
    let pool = write_pool(ctx)?;

    sqlx::query("DELETE FROM store_product_list WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool)
        .await?;

    // The detail row survives so links from past orders still resolve; the
    // storefront route filters on `published`.
    sqlx::query("UPDATE store_product_detail SET published = 0 WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool)
        .await?;

    sqlx::query("UPDATE admin_product_list SET status = 'archived' WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool)
        .await?;

    Ok(())
}

/// The read-model subscription, unstarted.
///
/// Exposed so tests and the demo app's end-to-end run can drive it
/// deterministically with `.no_retry().run_once(&executor)` instead of racing a
/// background task.
pub fn read_models_subscription(write_pool: SqlitePool) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(READ_MODELS_SUBSCRIPTION)
        .data(write_pool)
        .handler(on_product_imported())
        .handler(on_product_price_set())
        .handler(on_product_published())
        .handler(on_product_archived())
        .strict()
}

/// Spawn every background subscription this crate owns.
///
/// The caller keeps the handles and calls `shutdown()` on them.
pub async fn start_subscriptions(state: &CatalogState) -> anyhow::Result<Vec<Subscription>> {
    let read_models = read_models_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    tracing::info!(
        subscription = READ_MODELS_SUBSCRIPTION,
        "catalog subscriptions started"
    );
    Ok(vec![read_models])
}
