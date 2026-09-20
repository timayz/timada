//! What the storefront lists: `catalog_listing`, one row per product with
//! everything a product card, a filter or a sort needs — the catalog's own
//! data, the price (`timada-pricing`), what can be delivered
//! (`timada-inventory`) and the rating (`timada-review`) — plus a full-text
//! index. Fed by the `catalog-listing` subscription, which rewrites a
//! product's row with absolute values whenever anything about it changes, so
//! a redelivery changes nothing.
//!
//! A product is **on sale** — listed — while it is not archived and has a
//! price that was not withdrawn.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use timada_inventory::{
    StockLocation,
    aggregator::{StockReceived, StockReservationReleased, StockReserved, StockReturned},
    load_stock_availability, stock_item_id,
};
use timada_pricing::{
    aggregator::{ProductPriceChanged, ProductPriceListed, ProductPriceWithdrawn},
    load_product_price, price_id,
};
use timada_review::{aggregator::ReviewPublished, load_review_details};

use crate::{
    aggregator::{
        CategoryMoved, CategoryRenamed, ProductArchived, ProductCategorised, ProductCreated,
        ProductDescribed, ProductMediaAdded,
    },
    command::{Command, MAX_CATEGORY_DEPTH},
    query::load_product_page,
    value_object::MediaKind,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const LISTING_SUBSCRIPTION: &str = "catalog-listing";

/// Not strict: it follows a few events of five aggregates across four
/// contexts, not every event of one.
pub fn listing_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(LISTING_SUBSCRIPTION)
        .handler(on_product_created())
        .handler(on_product_described())
        .handler(on_product_media_added())
        .handler(on_product_categorised())
        .handler(on_product_archived())
        .handler(on_price_listed())
        .handler(on_price_changed())
        .handler(on_price_withdrawn())
        .handler(on_stock_received())
        .handler(on_stock_returned())
        .handler(on_stock_reserved())
        .handler(on_stock_released())
        .handler(on_review_published())
        .handler(on_category_renamed())
        .handler(on_category_moved())
}

/// A product as a listing shows it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ListingRow {
    pub product_id: String,
    pub sku: String,
    pub name: String,
    pub brand_name: String,
    pub brand_slug: String,
    pub category_id: Option<String>,
    pub short_description: String,
    pub thumbnail_url: Option<String>,
    pub thumbnail_alt: Option<String>,
    /// The listed price, all taxes included, in minor units.
    pub price_minor: i64,
    pub currency: String,
    /// What the warehouse can deliver.
    pub available: i64,
    /// The average of the published reviews, when there is one.
    pub rating_avg: Option<f64>,
    pub review_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListingSort {
    /// Best match first when there is a search, by name otherwise.
    #[default]
    Relevance,
    PriceAsc,
    PriceDesc,
    /// Best rated first; the more reviews the better among equals.
    Rating,
    Newest,
}

/// What to list. Every filter narrows; none set lists everything on sale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingQuery {
    /// Free text: every word must match (as a prefix) the name, the brand,
    /// the category, the SKU or the key features.
    pub q: Option<String>,
    /// This category and everything under it.
    pub category_id: Option<String>,
    /// Any of these brands.
    pub brand_slugs: Vec<String>,
    pub price_min_minor: Option<i64>,
    pub price_max_minor: Option<i64>,
    pub in_stock: bool,
    /// At least that many stars on average.
    pub min_rating: Option<u8>,
    pub sort: ListingSort,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListingQuery {
    fn default() -> Self {
        Self {
            q: None,
            category_id: None,
            brand_slugs: Vec::new(),
            price_min_minor: None,
            price_max_minor: None,
            in_stock: false,
            min_rating: None,
            sort: ListingSort::default(),
            limit: 24,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandFacet {
    pub slug: String,
    pub name: String,
    pub count: i64,
}

/// What the filters would give, each counted with every *other* filter
/// applied — so picking a second brand, say, is always possible.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingFacets {
    /// By name.
    pub brands: Vec<BrandFacet>,
    /// The cheapest and the dearest price, in minor units.
    pub price_range: Option<(i64, i64)>,
    pub in_stock: i64,
    /// How many products average at least 4, 3, 2 and 1 stars, in that order.
    pub rated_at_least: [(u8, i64); 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListingPage {
    pub rows: Vec<ListingRow>,
    /// How many products match, all pages together.
    pub total: i64,
    pub facets: ListingFacets,
}

/// The full-text expression of a search: each word a quoted prefix, all
/// required. `None` when the text has no word in it.
fn fts_expression(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(12)
        .map(|word| format!("\"{word}\"*"))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// A filter a facet leaves out when it counts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    Brand,
    Price,
    Stock,
    Rating,
}

/// `FROM … WHERE …` for a query, leaving `without` out.
fn push_matching(
    sql: &mut QueryBuilder<Sqlite>,
    query: &ListingQuery,
    fts: Option<&str>,
    without: Option<Filter>,
) {
    sql.push(" FROM catalog_listing l");
    if fts.is_some() {
        sql.push(" JOIN catalog_listing_fts ON catalog_listing_fts.rowid = l.rowid");
    }
    sql.push(" WHERE l.archived = 0 AND l.price_minor IS NOT NULL");
    if let Some(fts) = fts {
        sql.push(" AND catalog_listing_fts MATCH ").push_bind(fts);
    }
    if let Some(category_id) = &query.category_id {
        sql.push(" AND instr(l.category_trail, ")
            .push_bind(format!("/{category_id}/"))
            .push(") > 0");
    }
    if without != Some(Filter::Brand) && !query.brand_slugs.is_empty() {
        sql.push(" AND l.brand_slug IN (");
        let mut slugs = sql.separated(", ");
        for slug in &query.brand_slugs {
            slugs.push_bind(slug);
        }
        sql.push(")");
    }
    if without != Some(Filter::Price) {
        if let Some(min) = query.price_min_minor {
            sql.push(" AND l.price_minor >= ").push_bind(min);
        }
        if let Some(max) = query.price_max_minor {
            sql.push(" AND l.price_minor <= ").push_bind(max);
        }
    }
    if without != Some(Filter::Stock) && query.in_stock {
        sql.push(" AND l.available > 0");
    }
    if without != Some(Filter::Rating)
        && let Some(stars) = query.min_rating
    {
        sql.push(" AND l.rating_avg >= ")
            .push_bind(f64::from(stars));
    }
}

/// The products on sale matching `query`: one page of them, how many there
/// are in all, and what each filter would give.
pub async fn search_listing(db: &SqlitePool, query: &ListingQuery) -> sqlx::Result<ListingPage> {
    // A search without a word in it is no search.
    let fts = query.q.as_deref().and_then(fts_expression);
    let fts = fts.as_deref();

    let mut page = QueryBuilder::<Sqlite>::new(
        "SELECT l.product_id, l.sku, l.name, l.brand_name, l.brand_slug, l.category_id,
                l.short_description, l.thumbnail_url, l.thumbnail_alt, l.price_minor, l.currency,
                l.available, l.rating_avg, l.review_count",
    );
    push_matching(&mut page, query, fts, None);
    page.push(match (query.sort, fts.is_some()) {
        // Name and brand weigh most, then the SKU, the category, the features.
        (ListingSort::Relevance, true) => {
            " ORDER BY bm25(catalog_listing_fts, 10.0, 6.0, 2.0, 8.0, 1.0), l.name COLLATE NOCASE, l.product_id"
        }
        (ListingSort::Relevance, false) => " ORDER BY l.name COLLATE NOCASE, l.product_id",
        (ListingSort::PriceAsc, _) => " ORDER BY l.price_minor, l.name COLLATE NOCASE, l.product_id",
        (ListingSort::PriceDesc, _) => " ORDER BY l.price_minor DESC, l.name COLLATE NOCASE, l.product_id",
        (ListingSort::Rating, _) => {
            " ORDER BY l.rating_avg DESC NULLS LAST, l.review_count DESC, l.name COLLATE NOCASE, l.product_id"
        }
        (ListingSort::Newest, _) => " ORDER BY l.created_at DESC, l.rowid DESC",
    });
    page.push(" LIMIT ")
        .push_bind(query.limit)
        .push(" OFFSET ")
        .push_bind(query.offset);
    let rows: Vec<ListingRow> = page.build_query_as().fetch_all(db).await?;

    let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(*)");
    push_matching(&mut count, query, fts, None);
    let total: i64 = count.build_query_scalar().fetch_one(db).await?;

    let mut brands =
        QueryBuilder::<Sqlite>::new("SELECT l.brand_slug, MIN(l.brand_name), COUNT(*)");
    push_matching(&mut brands, query, fts, Some(Filter::Brand));
    brands.push(" GROUP BY l.brand_slug ORDER BY MIN(l.brand_name) COLLATE NOCASE, l.brand_slug");
    let brands: Vec<(String, String, i64)> = brands.build_query_as().fetch_all(db).await?;

    let mut prices = QueryBuilder::<Sqlite>::new("SELECT MIN(l.price_minor), MAX(l.price_minor)");
    push_matching(&mut prices, query, fts, Some(Filter::Price));
    let (cheapest, dearest): (Option<i64>, Option<i64>) =
        prices.build_query_as().fetch_one(db).await?;

    let mut stock = QueryBuilder::<Sqlite>::new("SELECT COALESCE(SUM(l.available > 0), 0)");
    push_matching(&mut stock, query, fts, Some(Filter::Stock));
    let in_stock: i64 = stock.build_query_scalar().fetch_one(db).await?;

    let mut rated = QueryBuilder::<Sqlite>::new(
        "SELECT COALESCE(SUM(l.rating_avg >= 4), 0), COALESCE(SUM(l.rating_avg >= 3), 0),
                COALESCE(SUM(l.rating_avg >= 2), 0), COALESCE(SUM(l.rating_avg >= 1), 0)",
    );
    push_matching(&mut rated, query, fts, Some(Filter::Rating));
    let (four, three, two, one): (i64, i64, i64, i64) =
        rated.build_query_as().fetch_one(db).await?;

    Ok(ListingPage {
        rows,
        total,
        facets: ListingFacets {
            brands: brands
                .into_iter()
                .map(|(slug, name, count)| BrandFacet { slug, name, count })
                .collect(),
            price_range: cheapest.zip(dearest),
            in_stock,
            rated_at_least: [(4, four), (3, three), (2, two), (1, one)],
        },
    })
}

/// A brand on sale, by its slug: its name as the products spell it.
pub async fn brand_by_slug(db: &SqlitePool, slug: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT MIN(brand_name) FROM catalog_listing
         WHERE brand_slug = ? AND archived = 0 AND price_minor IS NOT NULL",
    )
    .bind(slug)
    .fetch_one(db)
    .await
}

/// Every product on sale with when it last changed — for a sitemap.
pub async fn listed_products(db: &SqlitePool) -> sqlx::Result<Vec<(String, i64)>> {
    sqlx::query_as(
        "SELECT product_id, updated_at FROM catalog_listing
         WHERE archived = 0 AND price_minor IS NOT NULL
         ORDER BY product_id",
    )
    .fetch_all(db)
    .await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

/// The ids (`/root/…/leaf/`) and the names of the way down to a category,
/// read from the event store: the category list may trail behind.
async fn category_trail<E: Executor>(
    executor: &E,
    category_id: Option<&str>,
) -> anyhow::Result<(String, String)> {
    let cmd = Command(executor);
    let mut steps = Vec::new();
    let mut next = category_id.map(str::to_owned);
    while let Some(id) = next {
        if steps.len() >= MAX_CATEGORY_DEPTH || steps.iter().any(|(seen, _)| *seen == id) {
            break;
        }
        let Some(category) = cmd.load_category(&id).await? else {
            break;
        };
        next = category.parent_id;
        steps.push((id, category.name));
    }
    steps.reverse();
    let ids: String = steps.iter().map(|(id, _)| format!("{id}/")).collect();
    let names = steps
        .into_iter()
        .map(|(_, name)| name)
        .collect::<Vec<_>>()
        .join(" ");
    Ok((format!("/{ids}"), names))
}

/// Rewrites a product's row — and its full-text entry — with what the four
/// contexts say about it *now*. `at` is the event's time.
async fn refresh_product<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    product_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    let Some(product) = load_product_page(executor, product_id).await? else {
        // A price or a stock item for something the catalog does not know.
        return Ok(());
    };
    let price = load_product_price(executor, price_id(product_id))
        .await?
        .filter(|price| !price.withdrawn)
        .map(|price| price.price_incl_tax);
    let available = load_stock_availability(
        executor,
        stock_item_id(product_id, &StockLocation::Warehouse),
    )
    .await?
    .map_or(0, |stock| stock.available);
    let (rating_avg, review_count): (Option<f64>, i64) = sqlx::query_as(
        "SELECT AVG(rating), COUNT(*) FROM catalog_listing_review WHERE product_id = ?",
    )
    .bind(product_id)
    .fetch_one(db)
    .await?;
    let (trail, category_names) = category_trail(executor, product.category_id.as_deref()).await?;
    let thumbnail = product
        .media
        .iter()
        .find(|media| media.kind == MediaKind::Image);

    let rowid: i64 = sqlx::query_scalar(
        "INSERT INTO catalog_listing
            (product_id, sku, name, brand_name, brand_slug, category_id, category_trail,
             short_description, thumbnail_url, thumbnail_alt, price_minor, currency, available,
             rating_avg, review_count, archived, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17)
         ON CONFLICT (product_id) DO UPDATE
         SET name = excluded.name, brand_name = excluded.brand_name,
             brand_slug = excluded.brand_slug, category_id = excluded.category_id,
             category_trail = excluded.category_trail,
             short_description = excluded.short_description,
             thumbnail_url = excluded.thumbnail_url, thumbnail_alt = excluded.thumbnail_alt,
             price_minor = excluded.price_minor, currency = excluded.currency,
             available = excluded.available, rating_avg = excluded.rating_avg,
             review_count = excluded.review_count, archived = excluded.archived,
             updated_at = MAX(updated_at, excluded.updated_at)
         RETURNING rowid",
    )
    .bind(&product.id)
    .bind(&product.sku)
    .bind(&product.name)
    .bind(&product.brand.name)
    .bind(&product.brand.slug)
    .bind(&product.category_id)
    .bind(&trail)
    .bind(&product.short_description)
    .bind(thumbnail.map(|media| media.url.as_str()))
    .bind(thumbnail.map(|media| media.alt.as_str()))
    .bind(price.as_ref().map(|price| price.minor))
    .bind(price.as_ref().map(|price| price.currency.as_str()))
    .bind(i64::from(available))
    .bind(rating_avg)
    .bind(review_count)
    .bind(product.archived)
    .bind(at as i64)
    .fetch_one(db)
    .await?;

    sqlx::query("DELETE FROM catalog_listing_fts WHERE rowid = ?")
        .bind(rowid)
        .execute(db)
        .await?;
    sqlx::query(
        "INSERT INTO catalog_listing_fts (rowid, name, brand, category, sku, features)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(rowid)
    .bind(&product.name)
    .bind(&product.brand.name)
    .bind(&category_names)
    .bind(&product.sku)
    .bind(format!(
        "{} {}",
        product.short_description,
        product.key_features.join(" ")
    ))
    .execute(db)
    .await?;
    Ok(())
}

async fn refresh<E: Executor>(
    ctx: &Context<'_, E>,
    product_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    refresh_product(ctx.executor, &pool(ctx)?, product_id, at).await
}

/// The price's stream is keyed by the price, not the product.
async fn refresh_priced<E: Executor>(
    ctx: &Context<'_, E>,
    price_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    match load_product_price(ctx.executor, price_id).await? {
        Some(price) => refresh(ctx, &price.product_id, at).await,
        None => Ok(()),
    }
}

/// Only the warehouse delivers; a shop's own shelf does not change a listing.
async fn refresh_stocked<E: Executor>(
    ctx: &Context<'_, E>,
    stock_item: &str,
    at: u64,
) -> anyhow::Result<()> {
    match load_stock_availability(ctx.executor, stock_item).await? {
        Some(stock) if stock.location == StockLocation::Warehouse => {
            refresh(ctx, &stock.product_id, at).await
        }
        _ => Ok(()),
    }
}

/// A category's name is searchable and its place decides what its products
/// are listed under: everything in the branch is rewritten.
async fn refresh_branch<E: Executor>(
    ctx: &Context<'_, E>,
    category_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let products: Vec<String> = sqlx::query_scalar(
        "SELECT product_id FROM catalog_listing WHERE instr(category_trail, ?) > 0",
    )
    .bind(format!("/{category_id}/"))
    .fetch_all(&db)
    .await?;
    for product_id in products {
        refresh_product(ctx.executor, &db, &product_id, at).await?;
    }
    Ok(())
}

#[evento::subscription]
async fn on_product_created<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductCreated>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_product_described<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductDescribed>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_product_media_added<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductMediaAdded>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_product_categorised<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductCategorised>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_product_archived<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductArchived>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_price_listed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductPriceListed>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.data.product_id, event.timestamp).await
}

#[evento::subscription]
async fn on_price_changed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductPriceChanged>,
) -> anyhow::Result<()> {
    refresh_priced(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_price_withdrawn<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductPriceWithdrawn>,
) -> anyhow::Result<()> {
    refresh_priced(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_stock_received<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReceived>,
) -> anyhow::Result<()> {
    refresh_stocked(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_stock_returned<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReturned>,
) -> anyhow::Result<()> {
    refresh_stocked(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_stock_reserved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReserved>,
) -> anyhow::Result<()> {
    refresh_stocked(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_stock_released<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReservationReleased>,
) -> anyhow::Result<()> {
    refresh_stocked(ctx, &event.aggregate_id, event.timestamp).await
}

/// A published review counts once, whatever is redelivered.
#[evento::subscription]
async fn on_review_published<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReviewPublished>,
) -> anyhow::Result<()> {
    let Some(review) = load_review_details(ctx.executor, &event.aggregate_id).await? else {
        return Ok(());
    };
    sqlx::query(
        "INSERT OR IGNORE INTO catalog_listing_review (review_id, product_id, rating)
         VALUES (?, ?, ?)",
    )
    .bind(&review.id)
    .bind(&review.product_id)
    .bind(i64::from(review.rating))
    .execute(&pool(ctx)?)
    .await?;
    refresh(ctx, &review.product_id, event.timestamp).await
}

#[evento::subscription]
async fn on_category_renamed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CategoryRenamed>,
) -> anyhow::Result<()> {
    refresh_branch(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_category_moved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CategoryMoved>,
) -> anyhow::Result<()> {
    refresh_branch(ctx, &event.aggregate_id, event.timestamp).await
}
