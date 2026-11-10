//! The subscription that keeps the local product rows current from the
//! store's catalog events. The handler reloads the product's write model from
//! the store and rewrites the row, so a repeated delivery changes nothing.
//! The store answers with the stream's head, not the event: when a stream has
//! several events to deliver (a catch-up replays a product's whole history;
//! a reprice touches every variant in a row), the first of them writes the
//! rows from the head and the others find them noted as written
//! ([`Reflected`]) and cost nothing. Every handler writes in one transaction
//! of the local database ([`Db::write`]), run alongside the pages' reads and
//! run again whole should another writer change the same rows. The columns a
//! search reads, the option rows and the category memberships are written
//! only when they changed: the index behind the search columns builds a
//! segment for every write that names them, whatever the values.
//!
//! What a category counts, and the products its page lists, are kept on the
//! rows so the menu and a collection page read them instead of computing them
//! on each request. A product's change moves only its own rows and the
//! counts of the categories it reaches ([`file_product`]); a category moved
//! in the tree or archived changes what every page below and above lists, so
//! that works everything out again ([`refresh_category_counts`]) and writes
//! what differs.
//!
//! The same subscription follows the reviews: a published one is copied, one
//! rejected leaves, and its product's count and average are worked out again
//! with it.
//!
//! And the shop's pages: a published one is copied, one unpublished or
//! deleted leaves. And its announcements: the shown ones are copied whole.
//! And its default delivery time, written into every product giving none.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::aggregator::category::Category;
use timada_catalog::aggregator::stock::StockItem;
use timada_catalog::aggregator::{DeliveryDefault, Product};
use timada_catalog::{
    CategoryLink, CategoryState, ProductState, StockState, StockSummary, category_counts,
    category_projection, delivery_default_projection, product_projection, stock_projection,
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
        .data(Reflected::default())
        .handler(on_product_event())
        .handler(on_category_event())
        .handler(on_stock_event())
        .handler(on_review_event())
        .handler(crate::pages::on_page_event())
        .handler(crate::announcements::on_announcements_event())
        .handler(on_delivery_default_event())
        .any_routing_key()
}

/// What the local rows already reflect of each stream: the version of the
/// write model they were last written from.
///
/// A handler reloads the write model from the store, which answers with the
/// stream's head, not the event's own version. When a stream has several
/// events to deliver (a catch-up replays a product's whole history, seventy
/// events for an imported one; a reprice touches every variant in a row),
/// the first of them writes the rows from the head and notes it here; the
/// others, at or before that version, find the rows written and cost
/// nothing: no read of the store, no transaction. An event past what is
/// noted is new and loads again.
///
/// The note is made once the rows are committed, so a failed write is
/// retried in full. It lives in memory: after a restart the first event of
/// each stream loads once more, and a redelivery finds the same head and
/// writes the same rows. Shared by every handler of the subscription, keyed
/// by stream.
#[derive(Clone, Default)]
pub(crate) struct Reflected(Arc<Mutex<HashMap<String, u16>>>);

impl Reflected {
    /// How many streams are noted before the notes are let go of. A copy of a
    /// large shop notes every product, variant and stock item once; past
    /// this, forgetting costs one load per stream, and bounds the memory.
    const CAPACITY: usize = 500_000;

    fn key(event: &evento::Event) -> String {
        format!("{}/{}", event.aggregate_type, event.aggregate_id)
    }

    /// Whether the rows already reflect this event: it is at or before the
    /// version they were last written from.
    pub(crate) fn covers(&self, event: &evento::Event) -> bool {
        self.covers_at(&Self::key(event), event.version)
    }

    /// Notes that the rows of the event's stream now reflect `version`.
    pub(crate) fn note(&self, event: &evento::Event, version: u16) {
        self.note_at(Self::key(event), version);
    }

    fn covers_at(&self, key: &str, version: u16) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .is_some_and(|&written| version <= written)
    }

    fn note_at(&self, key: String, version: u16) {
        let mut noted = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if noted.len() >= Self::CAPACITY {
            noted.clear();
        }
        noted.insert(key, version);
    }
}

#[evento::subscription_all]
async fn on_product_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Product>,
) -> anyhow::Result<()> {
    let reflected: Reflected = context.extract();
    if reflected.covers(&event) {
        return Ok(());
    }
    let db: Db = context.extract();
    let Some(product) = product_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if product.deleted {
        delete_product_rows(&db, &product.id).await?;
    } else {
        upsert_product(&db, &product).await?;
    }
    reflected.note(&event, product.aggregate_version);
    Ok(())
}

#[evento::subscription_all]
async fn on_delivery_default_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<DeliveryDefault>,
) -> anyhow::Result<()> {
    let reflected: Reflected = context.extract();
    if reflected.covers(&event) {
        return Ok(());
    }
    let db: Db = context.extract();
    let state = delivery_default_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?;
    let days = state.as_ref().and_then(|state| state.days);
    db.write(&days, |tx, days| {
        Box::pin(async move {
            execute(tx, "DELETE FROM storefront_delivery_default", ()).await?;
            if let Some(days) = days {
                execute(
                    tx,
                    "INSERT INTO storefront_delivery_default (id, min_days, max_days)
                     VALUES (1, ?1, ?2)",
                    (i64::from(days.min), i64::from(days.max)),
                )
                .await?;
            }
            execute(
                tx,
                "UPDATE storefront_products SET
                    delivery_min_days = coalesce(own_delivery_min_days, ?1),
                    delivery_max_days = coalesce(own_delivery_max_days, ?2)",
                (
                    days.map(|days| i64::from(days.min)),
                    days.map(|days| i64::from(days.max)),
                ),
            )
            .await?;
            Ok(())
        })
    })
    .await?;
    if let Some(state) = state {
        reflected.note(&event, state.aggregate_version);
    }
    Ok(())
}

/// Drops every row of a deleted product (its own, its options, its category
/// memberships and its stock) and takes it out of the counts and the pages
/// that listed it. Nothing to drop is fine: delivery may repeat.
async fn delete_product_rows(db: &Db, product_id: &str) -> anyhow::Result<()> {
    db.write(product_id, |tx, product_id| {
        Box::pin(async move {
            let was_counted: Option<(bool,)> = fetch_optional(
                tx,
                "SELECT status = 'published' FROM storefront_products WHERE id = ?1",
                (product_id,),
            )
            .await?;
            let was_in = memberships_of(tx, product_id).await?;
            file_product(
                tx,
                product_id,
                was_counted.is_some_and(|(counted,)| counted),
                &was_in,
                false,
                &BTreeSet::new(),
            )
            .await?;
            for statement in [
                "DELETE FROM storefront_product_categories WHERE product_id = ?1",
                "DELETE FROM storefront_product_options WHERE product_id = ?1",
                "DELETE FROM storefront_stock WHERE product_id = ?1",
                "DELETE FROM storefront_products WHERE id = ?1",
            ] {
                execute(tx, statement, (product_id,)).await?;
            }
            Ok(())
        })
    })
    .await
}

#[evento::subscription_all]
async fn on_stock_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<StockItem>,
) -> anyhow::Result<()> {
    let reflected: Reflected = context.extract();
    if reflected.covers(&event) {
        return Ok(());
    }
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
    upsert_stock(&db, &stock, product.as_ref()).await?;
    reflected.note(&event, stock.aggregate_version);
    Ok(())
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
        for variant in product.sold_variants() {
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
    let reflected: Reflected = context.extract();
    if reflected.covers(&event) {
        return Ok(());
    }
    let db: Db = context.extract();
    let Some(category) = category_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_category(&db, &category).await?;
    reflected.note(&event, category.aggregate_version);
    Ok(())
}

#[evento::subscription_all]
async fn on_review_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Review>,
) -> anyhow::Result<()> {
    let reflected: Reflected = context.extract();
    if reflected.covers(&event) {
        return Ok(());
    }
    let db: Db = context.extract();
    let Some(review) = review_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_review(&db, &review).await?;
    reflected.note(&event, review.aggregate_version);
    Ok(())
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

/// The categories a product's rows file it under, in id order.
async fn memberships_of(tx: &Connection, product_id: &str) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String,)> = fetch_all(
        tx,
        "SELECT category_id FROM storefront_product_categories
         WHERE product_id = ?1 ORDER BY category_id",
        (product_id,),
    )
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The product row, the categories it is in, the counts they change and what
/// a search finds it by, in one transaction: a page never sees a product
/// filed half-way. The search columns (`slug_search`, `name_search`) are
/// left out of the write when their values are already in the row: a write
/// that names them makes the index behind them build and commit a segment,
/// whatever the values, and most events of a product (a picture, a variant,
/// a price) change neither. The option rows and the memberships are written
/// again only when they differ from the rows, for the same reason: an
/// imported product has dozens of values, and a price moves none of them.
async fn upsert_product(db: &Db, product: &ProductState) -> anyhow::Result<()> {
    db.write(product, |tx, product| {
        Box::pin(async move {
            let was: Option<(bool, String, String)> = fetch_optional(
                tx,
                "SELECT status = 'published', slug_search, name_search FROM storefront_products
                 WHERE id = ?1",
                (product.id.as_str(),),
            )
            .await?;
            let was_counted = was.as_ref().is_some_and(|(counted, _, _)| *counted);
            let was_in = memberships_of(tx, &product.id).await?;
            let slug_search = search_key(&product.slug);
            let name_search = search_key(&product.name);
            let options_json = options_json(product)?;
            // Twenty values: more than a tuple of parameters holds. The same
            // numbering serves the insert and the update, the two search
            // keys last so the update can leave them off.
            let mut values = vec![
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
                Value::from(options_json.as_str()),
                Value::from(variants_json(product)?),
                Value::from(attributes_json(product)?),
                Value::from(product.price_range().map(|(min, _, _)| min as i64)),
                Value::from(product.price_range().map(|(_, max, _)| max as i64)),
                Value::from(product.delivery_days.map(|days| i64::from(days.min))),
                Value::from(product.delivery_days.map(|days| i64::from(days.max))),
            ];
            match &was {
                None => {
                    values.push(Value::from(slug_search.as_str()));
                    values.push(Value::from(name_search.as_str()));
                    execute(tx, PRODUCT_INSERT, params_from_iter(values)).await?;
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
            // The option rows, as the table keeps them: one per pair of keys,
            // the first of two values folding to one.
            let options: Vec<crate::ProductOption> = serde_json::from_str(&options_json)?;
            let mut wanted: Vec<(String, String, String, String)> = Vec::new();
            for row in option_rows(&options) {
                if !wanted.iter().any(|(name_key, value_key, _, _)| {
                    *name_key == row.name_key && *value_key == row.value_key
                }) {
                    wanted.push((row.name_key, row.value_key, row.name, row.value));
                }
            }
            wanted.sort();
            let were: Vec<(String, String, String, String)> = fetch_all(
                tx,
                "SELECT name_key, value_key, name, value FROM storefront_product_options
                 WHERE product_id = ?1 ORDER BY name_key, value_key",
                (product.id.as_str(),),
            )
            .await?;
            if were != wanted {
                execute(
                    tx,
                    "DELETE FROM storefront_product_options WHERE product_id = ?1",
                    (product.id.as_str(),),
                )
                .await?;
                for (name_key, value_key, name, value) in &wanted {
                    execute(
                        tx,
                        INSERT_OPTION,
                        (
                            product.id.as_str(),
                            name_key.as_str(),
                            value_key.as_str(),
                            name.as_str(),
                            value.as_str(),
                        ),
                    )
                    .await?;
                }
            }
            if !was_in.iter().eq(product.categories.iter()) {
                execute(
                    tx,
                    "DELETE FROM storefront_product_categories WHERE product_id = ?1",
                    (product.id.as_str(),),
                )
                .await?;
                for category_id in &product.categories {
                    execute(
                        tx,
                        "INSERT INTO storefront_product_categories (product_id, category_id)
                         VALUES (?1, ?2)",
                        (product.id.as_str(), category_id.as_str()),
                    )
                    .await?;
                }
            }
            let is_counted = product.status.as_str() == "published";
            file_product(
                tx,
                &product.id,
                was_counted,
                &was_in,
                is_counted,
                &product.categories,
            )
            .await?;
            // A variant added or removed changes what is counted.
            refresh_stock(tx, product).await?;
            Ok(())
        })
    })
    .await
}

/// The row of a new product, every column: `?17` and `?18` are the search
/// keys; the delivery time shown is the product's own, else the shop's
/// default.
const PRODUCT_INSERT: &str = "INSERT INTO storefront_products (id, slug, name, description,
    amount_cents, currency, status, updated_at, images_json, options_json, variants_json,
    attributes_json, min_amount_cents, max_amount_cents, own_delivery_min_days,
    own_delivery_max_days, slug_search, name_search, delivery_min_days, delivery_max_days)
 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
    coalesce(?15, (SELECT min_days FROM storefront_delivery_default)),
    coalesce(?16, (SELECT max_days FROM storefront_delivery_default)))";

/// The row of a product written again, every column but the two the search
/// index reads (`slug_search`, `name_search`): they stay out of the write,
/// and so does the segment it would build.
const PRODUCT_UPDATE: &str = "UPDATE storefront_products SET
    slug = ?2, name = ?3, description = ?4, amount_cents = ?5, currency = ?6, status = ?7,
    updated_at = ?8, images_json = ?9, options_json = ?10, variants_json = ?11,
    attributes_json = ?12, min_amount_cents = ?13, max_amount_cents = ?14,
    own_delivery_min_days = ?15, own_delivery_max_days = ?16,
    delivery_min_days = coalesce(?15, (SELECT min_days FROM storefront_delivery_default)),
    delivery_max_days = coalesce(?16, (SELECT max_days FROM storefront_delivery_default))
 WHERE id = ?1";

/// [`PRODUCT_UPDATE`] with the two search columns too, for a renamed
/// product.
const PRODUCT_UPDATE_SEARCHED: &str = "UPDATE storefront_products SET
    slug = ?2, name = ?3, description = ?4, amount_cents = ?5, currency = ?6, status = ?7,
    updated_at = ?8, images_json = ?9, options_json = ?10, variants_json = ?11,
    attributes_json = ?12, min_amount_cents = ?13, max_amount_cents = ?14,
    own_delivery_min_days = ?15, own_delivery_max_days = ?16,
    delivery_min_days = coalesce(?15, (SELECT min_days FROM storefront_delivery_default)),
    delivery_max_days = coalesce(?16, (SELECT max_days FROM storefront_delivery_default)),
    slug_search = ?17, name_search = ?18
 WHERE id = ?1";

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

/// The ways it varies that shoppers choose, with the values on sale, as the
/// local rows carry them: an option the merchant hid is not there at all.
fn options_json(product: &ProductState) -> anyhow::Result<String> {
    let options: Vec<crate::ProductOption> = product
        .shown_options()
        .map(|option| crate::ProductOption {
            id: option.id.clone(),
            name: option.name.clone(),
            values: product.values_on_sale(option).cloned().collect(),
            label: option.label.clone(),
            value_labels: option.value_labels.clone(),
        })
        .collect();
    Ok(serde_json::to_string(&options)?)
}

/// The combinations on sale, naming the options shoppers choose, as the local
/// rows carry them.
fn variants_json(product: &ProductState) -> anyhow::Result<String> {
    let variants: Vec<crate::Variant> = product
        .sold_variants()
        .map(|variant| crate::Variant {
            id: variant.id.clone(),
            selections: product
                .shown_selections(variant)
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

/// The category row, and every count worked out again when it is new, moved
/// or archived: each of those changes what the pages below and above list.
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

/// Files a product's change into the counts and the pages, touching only
/// what it changes.
///
/// A category's page lists the counted (published) products filed under it
/// and under every live category below it, each once; so a product reaches
/// a page through each of its categories and the unbroken run of live
/// ancestors above it (an archived category stops the climb: its own page
/// lists only what is filed straight under it). `subtree_count` is how many
/// products a page lists, `product_count` how many are filed straight under
/// the category; both move by the difference between the pages that listed
/// the product and those that list it now. The pages that listed it are read
/// back from the rows rather than worked out from where it was, so a repeated
/// delivery changes nothing. A category the copy does not know yet (the
/// product's event came before its own) is passed over here and counted when
/// the category arrives, which counts everything again.
///
/// `was_counted` and `was_in` are what the rows said before this change,
/// `is_counted` and `now_in` what the product says now.
async fn file_product(
    conn: &Connection,
    product_id: &str,
    was_counted: bool,
    was_in: &[String],
    is_counted: bool,
    now_in: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let links: Vec<CategoryLink> = fetch_all(
        conn,
        "SELECT id, parent_id, archived FROM storefront_categories",
        (),
    )
    .await?;
    let tree: HashMap<&str, &CategoryLink> =
        links.iter().map(|link| (link.id.as_str(), link)).collect();
    let listed_before: BTreeSet<String> = fetch_all::<(String,)>(
        conn,
        "SELECT category_id FROM storefront_category_products WHERE product_id = ?1",
        (product_id,),
    )
    .await?
    .into_iter()
    .map(|(id,)| id)
    .collect();
    let listed_now = if is_counted {
        pages_listing(&tree, now_in.iter().map(String::as_str))
    } else {
        BTreeSet::new()
    };
    for category_id in listed_before.difference(&listed_now) {
        execute(
            conn,
            "DELETE FROM storefront_category_products WHERE category_id = ?1 AND product_id = ?2",
            (category_id.as_str(), product_id),
        )
        .await?;
        execute(
            conn,
            "UPDATE storefront_categories SET subtree_count = subtree_count - 1 WHERE id = ?1",
            (category_id.as_str(),),
        )
        .await?;
    }
    for category_id in listed_now.difference(&listed_before) {
        execute(conn, INSERT_LISTING, (category_id.as_str(), product_id)).await?;
        execute(
            conn,
            "UPDATE storefront_categories SET subtree_count = subtree_count + 1 WHERE id = ?1",
            (category_id.as_str(),),
        )
        .await?;
    }
    if is_counted && listed_now.intersection(&listed_before).next().is_some() {
        // The pages that go on listing it sort it by keys that moved with it.
        execute(
            conn,
            "UPDATE storefront_category_products
             SET (updated_at, min_amount_cents, max_amount_cents) =
                (SELECT updated_at, min_amount_cents, max_amount_cents
                 FROM storefront_products WHERE id = ?1)
             WHERE product_id = ?1",
            (product_id,),
        )
        .await?;
    }
    let filed_before: BTreeSet<&str> = if was_counted {
        was_in
            .iter()
            .map(String::as_str)
            .filter(|id| tree.contains_key(id))
            .collect()
    } else {
        BTreeSet::new()
    };
    let filed_now: BTreeSet<&str> = if is_counted {
        now_in
            .iter()
            .map(String::as_str)
            .filter(|id| tree.contains_key(id))
            .collect()
    } else {
        BTreeSet::new()
    };
    for category_id in filed_before.difference(&filed_now) {
        execute(
            conn,
            "UPDATE storefront_categories SET product_count = product_count - 1 WHERE id = ?1",
            (*category_id,),
        )
        .await?;
    }
    for category_id in filed_now.difference(&filed_before) {
        execute(
            conn,
            "UPDATE storefront_categories SET product_count = product_count + 1 WHERE id = ?1",
            (*category_id,),
        )
        .await?;
    }
    Ok(())
}

/// Lists a product on a category's page, with the keys the page sorts by
/// copied from its row: binds the category, then the product.
const INSERT_LISTING: &str = "INSERT INTO storefront_category_products (category_id, product_id,
    updated_at, min_amount_cents, max_amount_cents, review_count, rating_x100)
 SELECT ?1, id, updated_at, min_amount_cents, max_amount_cents, review_count, rating_x100
 FROM storefront_products WHERE id = ?2";

/// The categories whose pages list a product filed under `categories`: each
/// of them the tree knows and, above a live one, every live ancestor up to
/// the first archived or unknown. The same reach as
/// [`category_counts`]: a live category under an archived one stands at
/// the root of its own tree there.
fn pages_listing<'a>(
    tree: &HashMap<&str, &CategoryLink>,
    categories: impl Iterator<Item = &'a str>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for id in categories {
        let Some(mut link) = tree.get(id).copied() else {
            continue;
        };
        out.insert(link.id.clone());
        while !link.archived {
            let Some(parent) = link.parent_id.as_deref().and_then(|p| tree.get(p).copied()) else {
                break;
            };
            // A page reached already was climbed from already; a loop in
            // the tree ends here too.
            if parent.archived || !out.insert(parent.id.clone()) {
                break;
            }
            link = parent;
        }
    }
    out
}

/// Works out the counts of every category and the rows each one's page
/// lists, from scratch, and writes what differs from the rows: what a
/// category moved in the tree or archived calls for, since either changes
/// what the pages below and above list, and what a category arriving after
/// the products filed under it calls for. A product's own change goes
/// through [`file_product`] instead. Returns how many rows it changed.
async fn refresh_category_counts(conn: &Connection) -> anyhow::Result<usize> {
    let links: Vec<CategoryLink> = fetch_all(
        conn,
        "SELECT id, parent_id, archived FROM storefront_categories",
        (),
    )
    .await?;
    // Two scans joined here rather than one SQL join: Turso answers that
    // join by walking every product once per link, which made a recount of
    // a 1 300-product shop take seconds.
    let unpublished: HashSet<String> = fetch_all::<(String,)>(
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
    // What is listed and should stay is taken out as it is met; what is
    // left over is listed by no page any more.
    let mut stale: HashSet<(String, String)> = fetch_all::<(String, String)>(
        conn,
        "SELECT category_id, product_id FROM storefront_category_products",
        (),
    )
    .await?
    .into_iter()
    .collect();
    let mut changed = 0;
    for (id, counts) in category_counts(links, &memberships) {
        changed += execute(
            conn,
            "UPDATE storefront_categories SET product_count = ?2, subtree_count = ?3
             WHERE id = ?1 AND (product_count <> ?2 OR subtree_count <> ?3)",
            (id.as_str(), counts.product_count, counts.subtree_count),
        )
        .await? as usize;
        for product_id in &counts.subtree_products {
            if stale.remove(&(id.clone(), product_id.clone())) {
                continue;
            }
            execute(conn, INSERT_LISTING, (id.as_str(), product_id.as_str())).await?;
            changed += 1;
        }
    }
    for (category_id, product_id) in stale {
        execute(
            conn,
            "DELETE FROM storefront_category_products WHERE category_id = ?1 AND product_id = ?2",
            (category_id.as_str(), product_id.as_str()),
        )
        .await?;
        changed += 1;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::migrate;

    async fn db() -> Db {
        let db = Db::from(
            timada_core::db::Turso::in_memory_concurrent()
                .await
                .unwrap(),
        );
        migrate(&db).await.unwrap();
        db
    }

    async fn category(db: &Db, id: &str, parent_id: Option<&str>, archived: bool) {
        db.execute(
            "INSERT INTO storefront_categories (id, slug, name, description, body, parent_id,
                position, archived, in_menu, updated_at)
             VALUES (?1, ?1, ?1, '', '', ?2, 0, ?3, 1, 1)",
            (id, parent_id, archived),
        )
        .await
        .unwrap();
    }

    /// A product row with its memberships, as the subscription leaves them
    /// before filing; `filed` runs the filing with what the rows said before.
    async fn product(db: &Db, id: &str, status: &str, categories: &[&str]) {
        db.execute(
            "INSERT INTO storefront_products (id, slug, name, description, status, updated_at,
                images_json, options_json, variants_json, attributes_json, slug_search,
                name_search)
             VALUES (?1, ?1, ?1, '', ?2, 1, '[]', '[]', '[]', '[]', ?1, ?1)
             ON CONFLICT (id) DO UPDATE SET status = excluded.status",
            (id, status),
        )
        .await
        .unwrap();
        db.execute(
            "DELETE FROM storefront_product_categories WHERE product_id = ?1",
            (id,),
        )
        .await
        .unwrap();
        for category_id in categories {
            db.execute(
                "INSERT INTO storefront_product_categories (product_id, category_id)
                 VALUES (?1, ?2)",
                (id, *category_id),
            )
            .await
            .unwrap();
        }
    }

    /// Files a product's change, then checks a full recount has nothing
    /// left to write.
    async fn file(db: &Db, id: &str, was: Option<(bool, &[&str])>, now: Option<(bool, &[&str])>) {
        let env = (id, was, now);
        db.write(&env, |tx, (id, was, now)| {
            Box::pin(async move {
                let (was_counted, was_in) = was.unwrap_or((false, &[]));
                let (is_counted, now_in) = now.unwrap_or((false, &[]));
                let was_in: Vec<String> = was_in.iter().map(|c| (*c).to_owned()).collect();
                let now_in: BTreeSet<String> = now_in.iter().map(|c| (*c).to_owned()).collect();
                file_product(tx, id, was_counted, &was_in, is_counted, &now_in).await?;
                assert_eq!(
                    refresh_category_counts(tx).await?,
                    0,
                    "filing {id} left the counts short of a full recount"
                );
                Ok(())
            })
        })
        .await
        .unwrap();
    }

    async fn counts(db: &Db) -> Vec<(String, i64, i64)> {
        db.fetch_all(
            "SELECT id, product_count, subtree_count FROM storefront_categories
             WHERE product_count <> 0 OR subtree_count <> 0 ORDER BY id",
            (),
        )
        .await
        .unwrap()
    }

    async fn listed(db: &Db, category_id: &str) -> Vec<String> {
        db.fetch_all::<(String,)>(
            "SELECT product_id FROM storefront_category_products WHERE category_id = ?1
             ORDER BY product_id",
            (category_id,),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|(id,)| id)
        .collect()
    }

    fn row(id: &str, product_count: i64, subtree_count: i64) -> (String, i64, i64) {
        (id.to_owned(), product_count, subtree_count)
    }

    /// Bedding > Mats > Travel, Toys beside them, and an archived Old under
    /// Bedding with a live Vintage below it.
    async fn tree() -> Db {
        let db = db().await;
        category(&db, "bedding", None, false).await;
        category(&db, "mats", Some("bedding"), false).await;
        category(&db, "travel", Some("mats"), false).await;
        category(&db, "toys", None, false).await;
        category(&db, "old", Some("bedding"), true).await;
        category(&db, "vintage", Some("old"), false).await;
        db
    }

    #[tokio::test]
    async fn filing_a_product_moves_the_counts_it_reaches_and_agrees_with_a_recount() {
        let db = tree().await;

        // Published two levels down: listed on its page and its ancestors'.
        product(&db, "p1", "published", &["travel"]).await;
        file(&db, "p1", None, Some((true, &["travel"]))).await;
        assert_eq!(
            counts(&db).await,
            [row("bedding", 0, 1), row("mats", 0, 1), row("travel", 1, 1)]
        );

        // In a category and its parent: one product on the parent's page.
        product(&db, "p2", "published", &["bedding", "mats"]).await;
        file(&db, "p2", None, Some((true, &["bedding", "mats"]))).await;
        assert_eq!(
            counts(&db).await,
            [row("bedding", 1, 2), row("mats", 1, 2), row("travel", 1, 1)]
        );
        assert_eq!(listed(&db, "bedding").await, ["p1", "p2"]);

        // A draft counts nowhere until it is published.
        product(&db, "p3", "draft", &["toys"]).await;
        file(&db, "p3", None, Some((false, &["toys"]))).await;
        assert_eq!(listed(&db, "toys").await, Vec::<String>::new());
        product(&db, "p3", "published", &["toys"]).await;
        file(&db, "p3", Some((false, &["toys"])), Some((true, &["toys"]))).await;
        assert_eq!(listed(&db, "toys").await, ["p3"]);

        // Under an archived category: its own page only, nothing climbs.
        product(&db, "p4", "published", &["old"]).await;
        file(&db, "p4", None, Some((true, &["old"]))).await;
        // Under a live category below an archived one: the climb stops at
        // the archived one.
        product(&db, "p5", "published", &["vintage"]).await;
        file(&db, "p5", None, Some((true, &["vintage"]))).await;
        assert_eq!(
            counts(&db).await,
            [
                row("bedding", 1, 2),
                row("mats", 1, 2),
                row("old", 1, 1),
                row("toys", 1, 1),
                row("travel", 1, 1),
                row("vintage", 1, 1),
            ]
        );

        // A category the copy does not know yet is passed over.
        product(&db, "p6", "published", &["toys", "unknown"]).await;
        file(&db, "p6", None, Some((true, &["toys", "unknown"]))).await;
        assert_eq!(listed(&db, "toys").await, ["p3", "p6"]);

        // Unpublished: off every page, out of every count.
        product(&db, "p1", "draft", &["travel"]).await;
        file(
            &db,
            "p1",
            Some((true, &["travel"])),
            Some((false, &["travel"])),
        )
        .await;
        assert_eq!(listed(&db, "bedding").await, ["p2"]);
        assert_eq!(listed(&db, "travel").await, Vec::<String>::new());

        // Moved from one branch to another.
        product(&db, "p2", "published", &["toys"]).await;
        file(
            &db,
            "p2",
            Some((true, &["bedding", "mats"])),
            Some((true, &["toys"])),
        )
        .await;
        assert_eq!(listed(&db, "bedding").await, Vec::<String>::new());
        assert_eq!(listed(&db, "toys").await, ["p2", "p3", "p6"]);

        // Deleted: its memberships are gone with it.
        db.execute(
            "DELETE FROM storefront_product_categories WHERE product_id = 'p5'",
            (),
        )
        .await
        .unwrap();
        db.execute("DELETE FROM storefront_products WHERE id = 'p5'", ())
            .await
            .unwrap();
        file(&db, "p5", Some((true, &["vintage"])), None).await;
        assert_eq!(counts(&db).await, [row("old", 1, 1), row("toys", 3, 3)]);

        // Filed again with nothing changed: nothing moves.
        file(&db, "p3", Some((true, &["toys"])), Some((true, &["toys"]))).await;
        assert_eq!(counts(&db).await, [row("old", 1, 1), row("toys", 3, 3)]);
    }

    #[tokio::test]
    async fn a_recount_writes_only_what_differs() {
        let db = tree().await;
        product(&db, "p1", "published", &["travel"]).await;
        product(&db, "p2", "published", &["toys"]).await;
        db.write(&(), |tx, ()| {
            Box::pin(async move {
                // Four pages list a product: three up the bedding branch,
                // one for the toy; plus the counts of those four.
                assert_eq!(refresh_category_counts(tx).await?, 8);
                assert_eq!(refresh_category_counts(tx).await?, 0);
                // Mats moves under Toys: the bedding page loses the mat, the
                // toys page gains it.
                execute(
                    tx,
                    "UPDATE storefront_categories SET parent_id = 'toys' WHERE id = 'mats'",
                    (),
                )
                .await?;
                assert_eq!(refresh_category_counts(tx).await?, 4);
                Ok(())
            })
        })
        .await
        .unwrap();
        assert_eq!(listed(&db, "bedding").await, Vec::<String>::new());
        assert_eq!(listed(&db, "toys").await, ["p1", "p2"]);
        assert_eq!(
            counts(&db).await,
            [row("mats", 0, 1), row("toys", 1, 2), row("travel", 1, 1)]
        );
    }

    #[test]
    fn the_rows_reflect_a_stream_up_to_the_version_written() {
        let reflected = Reflected::default();
        assert!(!reflected.covers_at("Product/p1", 1));
        reflected.note_at("Product/p1".to_owned(), 7);
        assert!(reflected.covers_at("Product/p1", 1));
        assert!(reflected.covers_at("Product/p1", 7));
        assert!(!reflected.covers_at("Product/p1", 8), "a later event loads");
        assert!(
            !reflected.covers_at("Stock/p1", 1),
            "another stream of the same id is another stream"
        );
        let mut event = evento::Event {
            aggregate_type: "Product".to_owned(),
            aggregate_id: "p1".to_owned(),
            version: 3,
            ..Default::default()
        };
        assert!(reflected.covers(&event));
        event.version = 9;
        assert!(!reflected.covers(&event));
        reflected.note(&event, 9);
        assert!(reflected.covers(&event));
    }
}
