//! The subscription that keeps `catalog_products` current for every tenant.
//!
//! The handler reloads the strict write model of the stream the event belongs
//! to and rewrites its row, so delivery may repeat (the subscription is
//! at-least-once) without the row drifting. When the stream already holds a
//! later event than the one delivered (a catch-up replays a product's whole
//! history, ten events or so), the row is written once, by the last of them:
//! the earlier ones are [`superseded`]. The row's full-text columns are
//! written only when the slug or the name changed, as the index behind them
//! rebuilds a segment for every write that touches them. When what a category
//! counts has changed, the counts of the tenant's categories are worked out
//! again in the same transaction, so a page reads them instead of computing
//! them. One subscription follows every tenant's streams: each row carries
//! the tenant of the event that made it, and the rows live in the store's own
//! database ([`Db`]).

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{
    Connection, Db, Value, execute, fetch_all, fetch_optional, params_from_iter, search_key,
};

use crate::aggregator::Product;
use crate::aggregator::category::Category;
use crate::aggregator::stock::StockItem;
use crate::command::{
    CategoryState, ProductState, StockState, StockSummary, category_projection, product_projection,
    stock_projection,
};
use crate::query::refresh_category_counts;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-catalog/read-models";

/// Builds the subscription over every tenant and routing key. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_product_event())
        .handler(on_category_event())
        .handler(on_stock_event())
        .any_routing_key()
}

/// Whether the stream the event belongs to has already moved past it: the
/// write model loaded for the event is at `state_version`, and when that is
/// later than the event's own version the subscription has the later events
/// ahead of it (it follows every routing key, in order) and the last of them
/// writes the row this one would. Skipping the write for this event leaves
/// nothing behind: a redelivery of it finds the same later state and skips
/// again, and a crash before the later events are handled resumes at them.
fn superseded(event: &evento::Event, state_version: u16) -> bool {
    event.version < state_version
}

#[evento::subscription_all]
async fn on_stock_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<StockItem>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(stock) = stock_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if superseded(&event, stock.aggregate_version) {
        return Ok(());
    }
    let product = product_projection()
        .load(&stock.product_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?;
    upsert_stock(&db, &event.tenant, &stock, product.as_ref()).await
}

/// Writes the row of one thing's stock, and the summary of its product when
/// the product is known.
pub async fn upsert_stock(
    db: &Db,
    tenant_id: &str,
    stock: &StockState,
    product: Option<&ProductState>,
) -> anyhow::Result<()> {
    let env = (tenant_id, stock, product);
    db.write(&env, |tx, (tenant_id, stock, product)| {
        Box::pin(async move {
            execute(
                tx,
                "INSERT INTO catalog_stock (tenant_id, product_id, variant_id, tracked, on_hand,
                    updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (tenant_id, product_id, variant_id) DO UPDATE SET
                    tracked = excluded.tracked, on_hand = excluded.on_hand,
                    updated_at = excluded.updated_at",
                (
                    *tenant_id,
                    stock.product_id.as_str(),
                    stock.variant_id.as_deref().unwrap_or(""),
                    stock.tracked,
                    i64::from(stock.on_hand),
                    stock.updated_at as i64,
                ),
            )
            .await?;
            if let Some(product) = product {
                refresh_stock_state(tx, tenant_id, product).await?;
            }
            Ok(())
        })
    })
    .await
}

/// Works out a product's stock summary again from its stock rows.
async fn refresh_stock_state(
    tx: &Connection,
    tenant_id: &str,
    product: &ProductState,
) -> anyhow::Result<()> {
    let rows: Vec<(String, bool, i64)> = fetch_all(
        tx,
        "SELECT variant_id, tracked, on_hand FROM catalog_stock
         WHERE tenant_id = ?1 AND product_id = ?2",
        (tenant_id, product.id.as_str()),
    )
    .await?;
    let summary = StockSummary::of(product, |variant_id| {
        rows.iter()
            .find(|(id, tracked, _)| *tracked && id == variant_id.unwrap_or(""))
            .map(|(_, _, on_hand)| (*on_hand).max(0) as u32)
    });
    execute(
        tx,
        "UPDATE catalog_products SET stock_state = ?3 WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, product.id.as_str(), summary.as_str()),
    )
    .await?;
    Ok(())
}

#[evento::subscription_all]
async fn on_product_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Product>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(product) = product_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if superseded(&event, product.aggregate_version) {
        return Ok(());
    }
    upsert_product(&db, &event.tenant, &product).await
}

#[evento::subscription_all]
async fn on_category_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Category>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(category) = category_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if superseded(&event, category.aggregate_version) {
        return Ok(());
    }
    upsert_category(&db, &event.tenant, &category).await
}

/// Writes the row of a product and its category memberships as the write
/// model sees them, in one transaction: the row, its memberships and the
/// category counts they change are never half-written. The full-text
/// columns (`slug_search`, `name_search`) are left out of the write when
/// their values are already in the row: a write that names them makes the
/// index behind them build and commit a segment, whatever the values, and
/// most events of a product (a picture, a variant, a price) change neither.
pub async fn upsert_product(
    db: &Db,
    tenant_id: &str,
    product: &ProductState,
) -> anyhow::Result<()> {
    let env = (tenant_id, product);
    db.write(&env, |tx, (tenant_id, product)| {
        Box::pin(async move {
            let was: Option<(bool, String, String)> = fetch_optional(
                tx,
                "SELECT status <> 'archived', slug_search, name_search FROM catalog_products
                 WHERE tenant_id = ?1 AND id = ?2",
                (*tenant_id, product.id.as_str()),
            )
            .await?;
            let was_counted = was.as_ref().map(|(counted, _, _)| *counted);
            let was_in: Vec<(String,)> = fetch_all(
                tx,
                "SELECT category_id FROM catalog_product_categories
                 WHERE tenant_id = ?1 AND product_id = ?2 ORDER BY category_id",
                (*tenant_id, product.id.as_str()),
            )
            .await?;
            let slug_search = search_key(&product.slug);
            let name_search = search_key(&product.name);
            // Eighteen values: more than a tuple of parameters holds. The
            // same numbering serves the insert and the update, the two
            // search keys last so the update can leave them off.
            let mut values = vec![
                Value::from(*tenant_id),
                Value::from(product.id.as_str()),
                Value::from(product.slug.as_str()),
                Value::from(product.name.as_str()),
                Value::from(product.description.as_str()),
                Value::from(product.price.as_ref().map(|p| p.amount_cents as i64)),
                // The currency everything on this product is priced in,
                // which for one sold only through its variants is theirs
                // rather than a base price's.
                Value::from(product.currency()),
                Value::from(product.status.as_str()),
                Value::from(product.created_by.as_str()),
                Value::from(product.created_at as i64),
                Value::from(product.updated_at as i64),
                Value::from(product.images.first().map(|image| image.file())),
                Value::from(product.images.len() as i64),
                Value::from(product.price_range().map(|(min, _, _)| min as i64)),
                Value::from(product.price_range().map(|(_, max, _)| max as i64)),
                Value::from(product.variants.len() as i64),
            ];
            match &was {
                None => {
                    values.push(Value::from(slug_search.as_str()));
                    values.push(Value::from(name_search.as_str()));
                    execute(
                        tx,
                        "INSERT INTO catalog_products (tenant_id, id, slug, name, description,
                            amount_cents, currency, status, created_by, created_at, updated_at,
                            thumbnail, image_count, min_amount_cents, max_amount_cents,
                            variant_count, slug_search, name_search)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                            ?16, ?17, ?18)",
                        params_from_iter(values),
                    )
                    .await?;
                }
                Some((_, was_slug_search, was_name_search)) => {
                    let searched_as_before =
                        *was_slug_search == slug_search && *was_name_search == name_search;
                    if !searched_as_before {
                        values.push(Value::from(slug_search.as_str()));
                        values.push(Value::from(name_search.as_str()));
                    }
                    execute(
                        tx,
                        if searched_as_before {
                            PRODUCT_UPDATE
                        } else {
                            PRODUCT_UPDATE_SEARCHED
                        },
                        params_from_iter(values),
                    )
                    .await?;
                }
            }
            let categories_as_before = was_in.iter().map(|(id,)| id).eq(product.categories.iter());
            if !categories_as_before {
                execute(
                    tx,
                    "DELETE FROM catalog_product_categories
                     WHERE tenant_id = ?1 AND product_id = ?2",
                    (*tenant_id, product.id.as_str()),
                )
                .await?;
                for category_id in &product.categories {
                    execute(
                        tx,
                        "INSERT INTO catalog_product_categories (tenant_id, product_id, category_id)
                         VALUES (?1, ?2, ?3)",
                        (*tenant_id, product.id.as_str(), category_id.as_str()),
                    )
                    .await?;
                }
            }
            let is_counted = product.status.as_str() != "archived";
            // A rename or a new price leaves every count as it was.
            if was_counted.unwrap_or(false) != is_counted || (is_counted && !categories_as_before) {
                refresh_category_counts(tx, tenant_id).await?;
            }
            // A variant added or removed changes what the summary counts.
            refresh_stock_state(tx, tenant_id, product).await?;
            Ok(())
        })
    })
    .await
}

/// The row of a product written again, every column but the two the
/// full-text index reads (`slug_search`, `name_search`): it stays out of the
/// write, and so does the segment it would build.
const PRODUCT_UPDATE: &str = "UPDATE catalog_products SET
    slug = ?3, name = ?4, description = ?5, amount_cents = ?6, currency = ?7, status = ?8,
    updated_at = ?11, thumbnail = ?12, image_count = ?13, min_amount_cents = ?14,
    max_amount_cents = ?15, variant_count = ?16
 WHERE tenant_id = ?1 AND id = ?2";

/// [`PRODUCT_UPDATE`] with the two full-text columns too, for a renamed
/// product.
const PRODUCT_UPDATE_SEARCHED: &str = "UPDATE catalog_products SET
    slug = ?3, name = ?4, description = ?5, amount_cents = ?6, currency = ?7, status = ?8,
    updated_at = ?11, thumbnail = ?12, image_count = ?13, min_amount_cents = ?14,
    max_amount_cents = ?15, variant_count = ?16, slug_search = ?17, name_search = ?18
 WHERE tenant_id = ?1 AND id = ?2";

/// Writes the row of a category as the write model sees it, and works the
/// counts out again when it is new, moved or archived.
pub async fn upsert_category(
    db: &Db,
    tenant_id: &str,
    category: &CategoryState,
) -> anyhow::Result<()> {
    let env = (tenant_id, category);
    db.write(&env, |tx, (tenant_id, category)| {
        Box::pin(async move {
            let was: Option<(Option<String>, bool)> = fetch_optional(
                tx,
                "SELECT parent_id, archived FROM catalog_categories
                 WHERE tenant_id = ?1 AND id = ?2",
                (*tenant_id, category.id.as_str()),
            )
            .await?;
            execute(
                tx,
                "INSERT INTO catalog_categories (tenant_id, id, slug, name, description,
                    parent_id, position, archived, in_menu, created_by, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET
                    slug = excluded.slug, name = excluded.name,
                    description = excluded.description,
                    parent_id = excluded.parent_id, position = excluded.position,
                    archived = excluded.archived, in_menu = excluded.in_menu,
                    updated_at = excluded.updated_at",
                (
                    *tenant_id,
                    category.id.as_str(),
                    category.slug.as_str(),
                    category.name.as_str(),
                    category.description.as_str(),
                    category.parent_id.as_deref(),
                    category.position as i64,
                    category.archived,
                    !category.hidden_from_menu,
                    category.created_by.as_str(),
                    category.created_at as i64,
                    category.updated_at as i64,
                ),
            )
            .await?;
            if was != Some((category.parent_id.clone(), category.archived)) {
                refresh_category_counts(tx, tenant_id).await?;
            }
            Ok(())
        })
    })
    .await
}
