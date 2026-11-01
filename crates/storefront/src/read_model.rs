//! The subscription that keeps the local product rows current from the
//! store's catalog events. The handler reloads the product's write model from
//! the store (its snapshot is local too) and rewrites the row, so a repeated
//! delivery changes nothing. Every handler writes in one transaction of the
//! local database ([`Db::write`]), run alongside the pages' reads and run
//! again whole should another writer change the same rows. When what a category counts has changed, every
//! category's counts, and the products each one's page lists, are worked out
//! again in the same transaction, so the menu and a collection page read them
//! instead of computing them on each request.
//!
//! The same subscription follows the reviews: a published one is copied, one
//! rejected leaves, and its product's count and average are worked out again
//! with it.
//!
//! And the shop's pages: a published one is copied, one unpublished or
//! deleted leaves.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::aggregator::Product;
use timada_catalog::aggregator::category::Category;
use timada_catalog::aggregator::stock::StockItem;
use timada_catalog::{
    CategoryLink, CategoryState, ProductState, StockState, StockSummary, category_counts,
    category_projection, product_projection, stock_projection,
};
use timada_core::db::{
    Connection, Db, Value, execute, fetch_all, fetch_optional, params_from_iter, search_key,
};
use timada_reviews::aggregator::Review;
use timada_reviews::{ReviewState, review_projection};

/// The cursor identity of the catalog subscription, in the local database.
pub const CATALOG_SUBSCRIPTION: &str = "storefront/catalog";

/// Builds the subscription over every routing key of the store. `db` is the
/// local database; start it with `.start(&executor)`.
pub fn catalog_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CATALOG_SUBSCRIPTION)
        .data(db)
        .handler(on_product_event())
        .handler(on_category_event())
        .handler(on_stock_event())
        .handler(on_review_event())
        .handler(crate::pages::on_page_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_product_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Product>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(product) = product_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_product(&db, &product).await
}

#[evento::subscription_all]
async fn on_stock_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<StockItem>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(stock) = stock_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let product = product_projection()
        .load(&stock.product_id)
        .execute(context.executor)
        .await?;
    upsert_stock(&db, &stock, product.as_ref()).await
}

/// The stock row of one thing, and what its product's row says of it.
async fn upsert_stock(
    db: &Db,
    stock: &StockState,
    product: Option<&ProductState>,
) -> anyhow::Result<()> {
    let env = (stock, product);
    db.write(&env, |tx, (stock, product)| {
        Box::pin(async move {
            execute(
                tx,
                "INSERT INTO storefront_stock (product_id, variant_id, tracked, on_hand)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (product_id, variant_id) DO UPDATE SET
                    tracked = excluded.tracked, on_hand = excluded.on_hand",
                (
                    stock.product_id.as_str(),
                    stock.variant_id.as_deref().unwrap_or(""),
                    stock.tracked,
                    i64::from(stock.on_hand),
                ),
            )
            .await?;
            if let Some(product) = product {
                refresh_stock(tx, product).await?;
            }
            Ok(())
        })
    })
    .await
}

/// Works out what the product's row says of its stock from the stock rows:
/// `stock_json` maps each counted thing it still sells (`""` for a product
/// without variants) to what is on its shelf, `in_stock` whether anything of
/// it can be bought. What a checkout holds is left out: it lapses on its
/// own, and the cart and the payment check it exactly.
async fn refresh_stock(tx: &Connection, product: &ProductState) -> anyhow::Result<()> {
    let rows: Vec<(String, bool, i64)> = fetch_all(
        tx,
        "SELECT variant_id, tracked, on_hand FROM storefront_stock WHERE product_id = ?1",
        (product.id.as_str(),),
    )
    .await?;
    let on_hand = |variant_id: Option<&str>| {
        rows.iter()
            .find(|(id, tracked, _)| *tracked && id == variant_id.unwrap_or(""))
            .map(|(_, _, on_hand)| (*on_hand).max(0) as u32)
    };
    let mut counted = std::collections::BTreeMap::new();
    if product.variants.is_empty() {
        if let Some(left) = on_hand(None) {
            counted.insert(String::new(), left);
        }
    } else {
        for variant in &product.variants {
            if let Some(left) = on_hand(Some(&variant.id)) {
                counted.insert(variant.id.clone(), left);
            }
        }
    }
    let in_stock = StockSummary::of(product, on_hand) != StockSummary::SoldOut;
    execute(
        tx,
        "UPDATE storefront_products SET stock_json = ?2, in_stock = ?3 WHERE id = ?1",
        (
            product.id.as_str(),
            serde_json::to_string(&counted)?,
            in_stock,
        ),
    )
    .await?;
    Ok(())
}

#[evento::subscription_all]
async fn on_category_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Category>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(category) = category_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_category(&db, &category).await
}

#[evento::subscription_all]
async fn on_review_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Review>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(review) = review_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_review(&db, &review).await
}

/// Copies a published review (and drops one that is not), then counts its
/// product's reviews again, in one transaction.
async fn upsert_review(db: &Db, review: &ReviewState) -> anyhow::Result<()> {
    db.write(review, |tx, review| {
        Box::pin(async move {
            match review.published_at {
                Some(published_at) if review.is_published() => {
                    let photos: Vec<crate::ReviewPhoto> = review
                        .photos
                        .iter()
                        .map(|photo| crate::ReviewPhoto {
                            id: photo.id.clone(),
                            file: photo.file(),
                            width: photo.width,
                            height: photo.height,
                            // Filled in when a page reads the review.
                            url: String::new(),
                        })
                        .collect();
                    execute(
                        tx,
                        "INSERT INTO storefront_reviews (id, product_id, rating, title, body,
                            author_name, variant_label, photos_json, published_at, reply,
                            replied_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                         ON CONFLICT (id) DO UPDATE SET photos_json = excluded.photos_json,
                            reply = excluded.reply, replied_at = excluded.replied_at",
                        (
                            review.id.as_str(),
                            review.product_id.as_str(),
                            i64::from(review.rating),
                            review.title.as_deref(),
                            review.body.as_str(),
                            review.author_name.as_str(),
                            review.variant_label.as_deref(),
                            serde_json::to_string(&photos)?,
                            published_at as i64,
                            review.reply.as_deref(),
                            review.replied_at.map(|at| at as i64),
                        ),
                    )
                    .await?;
                }
                _ => {
                    execute(
                        tx,
                        "DELETE FROM storefront_reviews WHERE id = ?1",
                        (review.id.as_str(),),
                    )
                    .await?;
                }
            }
            // The average rounds half up; no review is 0 (a division by zero
            // is NULL).
            execute(
                tx,
                "UPDATE storefront_products SET (review_count, rating_x100) =
                    (SELECT COUNT(*), COALESCE((SUM(rating) * 100 + COUNT(*) / 2) / COUNT(*), 0)
                     FROM storefront_reviews WHERE product_id = ?1)
                 WHERE id = ?1",
                (review.product_id.as_str(),),
            )
            .await?;
            execute(
                tx,
                "UPDATE storefront_category_products SET (review_count, rating_x100) =
                    (SELECT review_count, rating_x100 FROM storefront_products WHERE id = ?1)
                 WHERE product_id = ?1",
                (review.product_id.as_str(),),
            )
            .await?;
            Ok(())
        })
    })
    .await
}

/// The product row, the categories it is in, the counts they change and what
/// a search finds it by, in one transaction: a page never sees a product
/// filed half-way.
async fn upsert_product(db: &Db, product: &ProductState) -> anyhow::Result<()> {
    db.write(product, |tx, product| {
        Box::pin(async move {
            let was: Option<(bool, String, String)> = fetch_optional(
                tx,
                "SELECT status = 'published', slug, name FROM storefront_products WHERE id = ?1",
                (product.id.as_str(),),
            )
            .await?;
            let was_counted = was.as_ref().map(|(counted, _, _)| *counted);
            let was_in: Vec<(String,)> = fetch_all(
                tx,
                "SELECT category_id FROM storefront_product_categories
                 WHERE product_id = ?1 ORDER BY category_id",
                (product.id.as_str(),),
            )
            .await?;
            let was_in: Vec<String> = was_in.into_iter().map(|(id,)| id).collect();
            execute(
                tx,
                "INSERT INTO storefront_products (id, slug, name, description, amount_cents,
                    currency, status, updated_at, images_json, options_json, variants_json,
                    attributes_json, min_amount_cents, max_amount_cents, slug_search,
                    name_search)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT (id) DO UPDATE SET
                    slug = excluded.slug, name = excluded.name, description = excluded.description,
                    amount_cents = excluded.amount_cents, currency = excluded.currency,
                    status = excluded.status, updated_at = excluded.updated_at,
                    images_json = excluded.images_json, options_json = excluded.options_json,
                    variants_json = excluded.variants_json,
                    attributes_json = excluded.attributes_json,
                    min_amount_cents = excluded.min_amount_cents,
                    max_amount_cents = excluded.max_amount_cents,
                    slug_search = excluded.slug_search, name_search = excluded.name_search",
                params_from_iter([
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
                    Value::from(product.updated_at as i64),
                    Value::from(images_json(product)?),
                    Value::from(options_json(product)?),
                    Value::from(variants_json(product)?),
                    Value::from(attributes_json(product)?),
                    Value::from(product.price_range().map(|(min, _, _)| min as i64)),
                    Value::from(product.price_range().map(|(_, max, _)| max as i64)),
                    Value::from(search_key(&product.slug)),
                    Value::from(search_key(&product.name)),
                ]),
            )
            .await?;
            execute(
                tx,
                "DELETE FROM storefront_product_categories WHERE product_id = ?1",
                (product.id.as_str(),),
            )
            .await?;
            execute(
                tx,
                "DELETE FROM storefront_product_options WHERE product_id = ?1",
                (product.id.as_str(),),
            )
            .await?;
            let options: Vec<crate::ProductOption> = serde_json::from_str(&options_json(product)?)?;
            for row in option_rows(&options) {
                execute(
                    tx,
                    INSERT_OPTION,
                    (
                        product.id.as_str(),
                        row.name_key.as_str(),
                        row.value_key.as_str(),
                        row.name.as_str(),
                        row.value.as_str(),
                    ),
                )
                .await?;
            }
            for category_id in &product.categories {
                execute(
                    tx,
                    "INSERT INTO storefront_product_categories (product_id, category_id)
                     VALUES (?1, ?2)",
                    (product.id.as_str(), category_id.as_str()),
                )
                .await?;
            }
            let is_counted = product.status.as_str() == "published";
            // A rename or a new price leaves every count as it was; only the
            // keys its categories' pages sort it by move.
            if was_counted.unwrap_or(false) != is_counted
                || (is_counted && !was_in.iter().eq(product.categories.iter()))
            {
                refresh_category_counts(tx).await?;
            } else if is_counted {
                execute(
                    tx,
                    "UPDATE storefront_category_products
                     SET (updated_at, min_amount_cents, max_amount_cents) =
                        (SELECT updated_at, min_amount_cents, max_amount_cents
                         FROM storefront_products WHERE id = ?1)
                     WHERE product_id = ?1",
                    (product.id.as_str(),),
                )
                .await?;
            }
            // A variant added or removed changes what is counted.
            refresh_stock(tx, product).await?;
            Ok(())
        })
    })
    .await
}

/// One row of `storefront_product_options`: a value an option of the product
/// takes, with the keys a filter compares.
pub(crate) struct OptionRow {
    pub name_key: String,
    pub value_key: String,
    pub name: String,
    pub value: String,
}

/// Inserts an [`OptionRow`] for a product: binds `product_id`, then the four
/// fields in order. Two values folding to one key keep the first.
pub(crate) const INSERT_OPTION: &str =
    "INSERT OR IGNORE INTO storefront_product_options (product_id, name_key, value_key, name, value)
     VALUES (?1, ?2, ?3, ?4, ?5)";

/// The option rows of a product, one per declared value; a value folding to
/// nothing (blank) is left out.
pub(crate) fn option_rows(options: &[crate::ProductOption]) -> Vec<OptionRow> {
    options
        .iter()
        .flat_map(|option| {
            let name_key = crate::facet_key(&option.name);
            option.values.iter().map(move |value| OptionRow {
                name_key: name_key.clone(),
                value_key: crate::facet_key(value),
                name: option.name.clone(),
                value: value.clone(),
            })
        })
        .filter(|row| !row.name_key.is_empty() && !row.value_key.is_empty())
        .collect()
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
async fn upsert_category(db: &Db, category: &CategoryState) -> anyhow::Result<()> {
    db.write(category, |tx, category| {
        Box::pin(async move {
            let was: Option<(Option<String>, bool)> = fetch_optional(
                tx,
                "SELECT parent_id, archived FROM storefront_categories WHERE id = ?1",
                (category.id.as_str(),),
            )
            .await?;
            execute(
                tx,
                "INSERT INTO storefront_categories (id, slug, name, description, parent_id,
                    position, archived, in_menu, updated_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (id) DO UPDATE SET
                    slug = excluded.slug, name = excluded.name,
                    description = excluded.description, body = excluded.body,
                    parent_id = excluded.parent_id, position = excluded.position,
                    archived = excluded.archived, in_menu = excluded.in_menu,
                    updated_at = excluded.updated_at",
                (
                    category.id.as_str(),
                    category.slug.as_str(),
                    category.name.as_str(),
                    category.description.as_str(),
                    category.parent_id.as_deref(),
                    category.position as i64,
                    category.archived,
                    !category.hidden_from_menu,
                    category.updated_at as i64,
                    category.body.as_str(),
                ),
            )
            .await?;
            if was != Some((category.parent_id.clone(), category.archived)) {
                refresh_category_counts(tx).await?;
            }
            Ok(())
        })
    })
    .await
}

/// Works out and stores the counts of every category, from the published
/// products filed under them, and which products each one's page lists.
async fn refresh_category_counts(conn: &Connection) -> anyhow::Result<()> {
    let links: Vec<CategoryLink> = fetch_all(
        conn,
        "SELECT id, parent_id, archived FROM storefront_categories",
        (),
    )
    .await?;
    // Two scans joined here rather than one SQL join: Turso answers that
    // join by walking every product once per link, which made a recount of
    // a 1 300-product shop take seconds.
    let unpublished: std::collections::HashSet<String> = fetch_all::<(String,)>(
        conn,
        "SELECT id FROM storefront_products WHERE status <> 'published'",
        (),
    )
    .await?
    .into_iter()
    .map(|(id,)| id)
    .collect();
    let memberships: Vec<(String, String)> = fetch_all::<(String, String)>(
        conn,
        "SELECT category_id, product_id FROM storefront_product_categories",
        (),
    )
    .await?
    .into_iter()
    .filter(|(_, product_id)| !unpublished.contains(product_id))
    .collect();
    execute(conn, "DELETE FROM storefront_category_products", ()).await?;
    for (id, counts) in category_counts(links, &memberships) {
        execute(
            conn,
            "UPDATE storefront_categories SET product_count = ?2, subtree_count = ?3
             WHERE id = ?1 AND (product_count <> ?2 OR subtree_count <> ?3)",
            (id.as_str(), counts.product_count, counts.subtree_count),
        )
        .await?;
        for product_id in &counts.subtree_products {
            execute(
                conn,
                "INSERT INTO storefront_category_products (category_id, product_id, updated_at,
                    min_amount_cents, max_amount_cents, review_count, rating_x100)
                 SELECT ?1, id, updated_at, min_amount_cents, max_amount_cents, review_count,
                    rating_x100
                 FROM storefront_products WHERE id = ?2",
                (id.as_str(), product_id.as_str()),
            )
            .await?;
        }
    }
    Ok(())
}
