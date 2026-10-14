//! The subscription that keeps the local product rows current from the
//! store's catalog events. The handler reloads the product's write model from
//! the store (its snapshot is local too) and rewrites the row, so a repeated
//! delivery changes nothing. When what a category counts has changed, every
//! category's counts are worked out again in the same transaction, so the
//! navigation menu reads them instead of computing them on each page.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_catalog::aggregator::Product;
use timada_catalog::aggregator::category::Category;
use timada_catalog::{
    CategoryLink, CategoryState, ProductState, category_counts, category_projection,
    product_projection,
};

/// The cursor identity of the catalog subscription, in the local database.
pub const CATALOG_SUBSCRIPTION: &str = "storefront/catalog";

/// Builds the subscription over every routing key of the store. `pool` is the
/// local database; start it with `.start(&executor)`.
pub fn catalog_subscription<E: Executor>(pool: SqlitePool) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CATALOG_SUBSCRIPTION)
        .data(pool)
        .handler(on_product_event())
        .handler(on_category_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_product_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Product>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(product) = product_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_product(&pool, &product).await
}

#[evento::subscription_all]
async fn on_category_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Category>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(category) = category_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_category(&pool, &category).await
}

/// The product row, the categories it is in and the counts they change, in
/// one transaction: a page never sees a product filed half-way.
async fn upsert_product(pool: &SqlitePool, product: &ProductState) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let was_counted: Option<bool> =
        sqlx::query_scalar("SELECT status = 'published' FROM storefront_products WHERE id = ?1")
            .bind(&product.id)
            .fetch_optional(&mut *tx)
            .await?;
    let was_in: Vec<String> = sqlx::query_scalar(
        "SELECT category_id FROM storefront_product_categories
         WHERE product_id = ?1 ORDER BY category_id",
    )
    .bind(&product.id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO storefront_products (id, slug, name, description, amount_cents, currency,
            status, updated_at, images_json, options_json, variants_json, attributes_json,
            min_amount_cents, max_amount_cents)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT (id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name, description = excluded.description,
            amount_cents = excluded.amount_cents, currency = excluded.currency,
            status = excluded.status, updated_at = excluded.updated_at,
            images_json = excluded.images_json, options_json = excluded.options_json,
            variants_json = excluded.variants_json,
            attributes_json = excluded.attributes_json,
            min_amount_cents = excluded.min_amount_cents,
            max_amount_cents = excluded.max_amount_cents",
    )
    .bind(&product.id)
    .bind(&product.slug)
    .bind(&product.name)
    .bind(&product.description)
    .bind(product.price.as_ref().map(|p| p.amount_cents as i64))
    // The currency everything on this product is priced in, which for one
    // sold only through its variants is theirs rather than a base price's.
    .bind(product.currency())
    .bind(product.status.as_str())
    .bind(product.updated_at as i64)
    .bind(images_json(product)?)
    .bind(options_json(product)?)
    .bind(variants_json(product)?)
    .bind(attributes_json(product)?)
    .bind(product.price_range().map(|(min, _, _)| min as i64))
    .bind(product.price_range().map(|(_, max, _)| max as i64))
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM storefront_product_categories WHERE product_id = ?1")
        .bind(&product.id)
        .execute(&mut *tx)
        .await?;
    for category_id in &product.categories {
        sqlx::query(
            "INSERT INTO storefront_product_categories (product_id, category_id)
             VALUES (?1, ?2)",
        )
        .bind(&product.id)
        .bind(category_id)
        .execute(&mut *tx)
        .await?;
    }
    let is_counted = product.status.as_str() == "published";
    // A rename or a new price leaves every count as it was.
    if was_counted.unwrap_or(false) != is_counted
        || (is_counted && !was_in.iter().eq(product.categories.iter()))
    {
        refresh_category_counts(&mut tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The gallery as the local rows carry it.
fn images_json(product: &ProductState) -> anyhow::Result<String> {
    let images: Vec<crate::Image> = product
        .images
        .iter()
        .map(|image| crate::Image {
            id: image.id.clone(),
            file: image.file(),
            mime: image.mime.clone(),
            alt: image.alt.clone(),
            width: image.width,
            height: image.height,
            // Filled in when a page reads the product: the address belongs
            // to the host's configuration, not to the row.
            url: String::new(),
        })
        .collect();
    Ok(serde_json::to_string(&images)?)
}

/// The ways it varies, as the local rows carry them.
fn options_json(product: &ProductState) -> anyhow::Result<String> {
    let options: Vec<crate::ProductOption> = product
        .options
        .iter()
        .map(|option| crate::ProductOption {
            id: option.id.clone(),
            name: option.name.clone(),
            values: option.values.clone(),
        })
        .collect();
    Ok(serde_json::to_string(&options)?)
}

/// The combinations it sells, as the local rows carry them.
fn variants_json(product: &ProductState) -> anyhow::Result<String> {
    let variants: Vec<crate::Variant> = product
        .variants
        .iter()
        .map(|variant| crate::Variant {
            id: variant.id.clone(),
            selections: variant
                .selections
                .iter()
                .map(|selection| crate::Selection {
                    option_id: selection.option_id.clone(),
                    value: selection.value.clone(),
                })
                .collect(),
            sku: variant.sku.clone(),
            amount_cents: variant.price.amount_cents as i64,
            currency: variant.price.currency.clone(),
            image_id: variant.image_id.clone(),
        })
        .collect();
    Ok(serde_json::to_string(&variants)?)
}

/// The facts in its table, as the local rows carry them.
fn attributes_json(product: &ProductState) -> anyhow::Result<String> {
    let attributes: Vec<crate::Attribute> = product
        .attributes
        .iter()
        .map(|attribute| crate::Attribute {
            name: attribute.name.clone(),
            value: attribute.value.clone(),
        })
        .collect();
    Ok(serde_json::to_string(&attributes)?)
}

/// The category row, and its counts worked out again when it is new, moved
/// or archived.
async fn upsert_category(pool: &SqlitePool, category: &CategoryState) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let was: Option<(Option<String>, bool)> =
        sqlx::query_as("SELECT parent_id, archived FROM storefront_categories WHERE id = ?1")
            .bind(&category.id)
            .fetch_optional(&mut *tx)
            .await?;
    sqlx::query(
        "INSERT INTO storefront_categories (id, slug, name, description, parent_id, position,
            archived, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name, description = excluded.description,
            parent_id = excluded.parent_id, position = excluded.position,
            archived = excluded.archived, updated_at = excluded.updated_at",
    )
    .bind(&category.id)
    .bind(&category.slug)
    .bind(&category.name)
    .bind(&category.description)
    .bind(category.parent_id.as_deref())
    .bind(category.position as i64)
    .bind(category.archived)
    .bind(category.updated_at as i64)
    .execute(&mut *tx)
    .await?;
    if was != Some((category.parent_id.clone(), category.archived)) {
        refresh_category_counts(&mut tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Works out and stores the counts of every category, from the published
/// products filed under them.
async fn refresh_category_counts(conn: &mut sqlx::SqliteConnection) -> sqlx::Result<()> {
    let links: Vec<CategoryLink> =
        sqlx::query_as("SELECT id, parent_id, archived FROM storefront_categories")
            .fetch_all(&mut *conn)
            .await?;
    let memberships: Vec<(String, String)> = sqlx::query_as(
        "SELECT pc.category_id, pc.product_id
         FROM storefront_product_categories pc
         JOIN storefront_products p ON p.id = pc.product_id
         WHERE p.status = 'published'",
    )
    .fetch_all(&mut *conn)
    .await?;
    for (id, counts) in category_counts(links, &memberships) {
        sqlx::query(
            "UPDATE storefront_categories SET product_count = ?2, subtree_count = ?3
             WHERE id = ?1 AND (product_count <> ?2 OR subtree_count <> ?3)",
        )
        .bind(&id)
        .bind(counts.product_count)
        .bind(counts.subtree_count)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
