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
//!
//! The versions of one article (a [`ProductFamily`](crate::aggregator::ProductFamily))
//! are **one card**: a search gives, of each family, the matching version
//! that comes first, with the cheapest price among those that match. Totals
//! and facets count cards. The reviews of a family's versions are about the
//! one article: every version carries the rating of them all.

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
    aggregator::{
        CurrencyPriceRemoved, CurrencyPriceSet, ProductPriceChanged, ProductPriceListed,
        ProductPriceWithdrawn,
    },
    load_product_price, price_id,
};
use timada_review::{aggregator::ReviewPublished, load_review_details};

use crate::{
    aggregator::{
        CategoryMoved, CategoryRenamed, FamilyRenamed, ProductArchived, ProductCategorised,
        ProductCreated, ProductDescribed, ProductJoinedFamily, ProductLeftFamily,
        ProductMediaAdded, ProductSpecified,
    },
    command::{Command, MAX_CATEGORY_DEPTH},
    query::load_product_page,
    value_object::{MediaKind, SpecKey},
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
        .handler(on_product_specified())
        .handler(on_product_categorised())
        .handler(on_product_archived())
        .handler(on_product_joined_family())
        .handler(on_product_left_family())
        .handler(on_family_renamed())
        .handler(on_price_listed())
        .handler(on_price_changed())
        .handler(on_price_withdrawn())
        .handler(on_currency_price_set())
        .handler(on_currency_price_removed())
        .handler(on_stock_received())
        .handler(on_stock_returned())
        .handler(on_stock_reserved())
        .handler(on_stock_released())
        .handler(on_review_published())
        .handler(on_category_renamed())
        .handler(on_category_moved())
}

/// A card of a listing: a product, or — for a family — the version of it
/// that comes first among those that match.
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
    /// The price, all taxes included, in minor units — in the currency the
    /// listing was asked for, or the one the product was listed in.
    ///
    /// Of a family: the cheapest among the versions that match — "from".
    pub price_minor: i64,
    pub currency: String,
    /// What the warehouse can deliver.
    pub available: i64,
    /// The average of the published reviews, when there is one.
    pub rating_avg: Option<f64>,
    pub review_count: i64,
    /// The family the product is a version of.
    pub family_id: Option<String>,
    pub family_name: Option<String>,
    /// How many versions of the family match — 1 for a product on its own.
    pub versions: i64,
    /// Whether the versions that match have different prices.
    pub price_varies: bool,
}

impl ListingRow {
    /// What the card is called: the family when it stands for several
    /// versions, the product otherwise.
    pub fn title(&self) -> &str {
        match &self.family_name {
            Some(family) if self.versions > 1 => family,
            _ => &self.name,
        }
    }
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
    /// List what is sold in this currency, at its price there: prices,
    /// the price filter and the price sort are all in it. `None`: every
    /// product on sale, at the price it was listed with.
    pub currency: Option<String>,
    pub in_stock: bool,
    /// At least that many stars on average.
    pub min_rating: Option<u8>,
    /// Lines of the technical sheet: every filter must match, by any of its
    /// values.
    pub specs: Vec<SpecFilter>,
    /// The specs to count values for ([`ListingFacets::specs`]) — those the
    /// category is filtered by.
    pub facet_specs: Vec<SpecKey>,
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
            currency: None,
            in_stock: false,
            min_rating: None,
            specs: Vec::new(),
            facet_specs: Vec::new(),
            sort: ListingSort::default(),
            limit: 24,
            offset: 0,
        }
    }
}

/// "`Dalle` › `Type` is `IPS` or `VA`".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecFilter {
    pub key: SpecKey,
    pub values: Vec<String>,
}

/// The values a spec takes among the products the other filters leave, with
/// how many products have each — numbers first, by size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecFacet {
    pub key: SpecKey,
    pub values: Vec<(String, i64)>,
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
    /// One per [`ListingQuery::facet_specs`] that has values, in that order.
    pub specs: Vec<SpecFacet>,
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

/// What makes a card: the family, or the product on its own.
const CARD: &str = "COALESCE(l.family_id, l.product_id)";

/// A filter a facet leaves out when it counts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter<'a> {
    Brand,
    Price,
    Stock,
    Rating,
    Spec(&'a SpecKey),
}

/// `FROM … WHERE …` for a query, leaving `without` out.
fn push_matching(
    sql: &mut QueryBuilder<Sqlite>,
    query: &ListingQuery,
    fts: Option<&str>,
    without: Option<Filter<'_>>,
) {
    sql.push(" FROM catalog_listing l");
    // Counting a spec's values: one row per product that has the spec.
    if let Some(Filter::Spec(key)) = without {
        sql.push(" JOIN catalog_listing_spec s ON s.product_id = l.product_id AND s.spec_group = ")
            .push_bind(key.group.clone())
            .push(" AND s.label = ")
            .push_bind(key.label.clone());
    }
    if fts.is_some() {
        sql.push(" JOIN catalog_listing_fts ON catalog_listing_fts.rowid = l.rowid");
    }
    // In one currency, a product is on sale when it has a price there.
    if let Some(currency) = &query.currency {
        sql.push(" JOIN catalog_listing_currency_price p ON p.product_id = l.product_id AND p.currency = ")
            .push_bind(currency.clone());
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
        let in_currency = query.currency.is_some();
        if let Some(min) = query.price_min_minor {
            sql.push(if in_currency {
                " AND p.price_minor >= "
            } else {
                " AND l.price_minor >= "
            })
            .push_bind(min);
        }
        if let Some(max) = query.price_max_minor {
            sql.push(if in_currency {
                " AND p.price_minor <= "
            } else {
                " AND l.price_minor <= "
            })
            .push_bind(max);
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
    for filter in &query.specs {
        if without == Some(Filter::Spec(&filter.key)) || filter.values.is_empty() {
            continue;
        }
        sql.push(
            " AND EXISTS (SELECT 1 FROM catalog_listing_spec f
               WHERE f.product_id = l.product_id AND f.spec_group = ",
        )
        .push_bind(filter.key.group.clone())
        .push(" AND f.label = ")
        .push_bind(filter.key.label.clone())
        .push(" AND f.value IN (");
        let mut values = sql.separated(", ");
        for value in &filter.values {
            values.push_bind(value.clone());
        }
        sql.push("))");
    }
}

/// The products on sale matching `query`: one page of them, how many there
/// are in all, and what each filter would give.
pub async fn search_listing(db: &SqlitePool, query: &ListingQuery) -> sqlx::Result<ListingPage> {
    // A search without a word in it is no search.
    let fts = query.q.as_deref().and_then(fts_expression);
    let fts = fts.as_deref();

    let in_currency = query.currency.is_some();
    // Innermost: what matches, under plain names. Then each family's versions
    // are ranked in the order asked for, and the first of each is its card.
    let mut page = QueryBuilder::<Sqlite>::new(
        "SELECT product_id, sku, name, brand_name, brand_slug, category_id, short_description,
                thumbnail_url, thumbnail_alt, from_minor AS price_minor, currency, available,
                rating_avg, review_count, family_id, family_name, versions,
                from_minor <> to_minor AS price_varies
         FROM (SELECT m.*,
                      ROW_NUMBER() OVER (PARTITION BY card ORDER BY ",
    );
    // Whatever the direction, a family is shown by its cheapest version.
    let within = match (query.sort, fts.is_some()) {
        (ListingSort::Relevance, true) => "score, sort_key, product_id",
        (ListingSort::Relevance, false) => "sort_key, product_id",
        (ListingSort::PriceAsc | ListingSort::PriceDesc, _) => "price, sort_key, product_id",
        (ListingSort::Rating, _) => {
            "rating_avg DESC NULLS LAST, review_count DESC, sort_key, product_id"
        }
        (ListingSort::Newest, _) => "created_at DESC, listing_rowid DESC",
    };
    page.push(within).push(
        ") AS place,
                      MIN(price) OVER (PARTITION BY card) AS from_minor,
                      MAX(price) OVER (PARTITION BY card) AS to_minor,
                      COUNT(*) OVER (PARTITION BY card) AS versions
               FROM (",
    );
    page.push(if in_currency {
        "SELECT l.product_id, l.sku, l.name, l.brand_name, l.brand_slug, l.category_id,
                l.short_description, l.thumbnail_url, l.thumbnail_alt,
                p.price_minor AS price, p.currency AS currency,
                l.available, l.rating_avg, l.review_count, l.family_id, l.family_name,
                l.created_at, l.rowid AS listing_rowid,
                COALESCE(NULLIF(l.sort_name, ''), lower(l.name)) AS sort_key, "
    } else {
        "SELECT l.product_id, l.sku, l.name, l.brand_name, l.brand_slug, l.category_id,
                l.short_description, l.thumbnail_url, l.thumbnail_alt,
                l.price_minor AS price, l.currency AS currency,
                l.available, l.rating_avg, l.review_count, l.family_id, l.family_name,
                l.created_at, l.rowid AS listing_rowid,
                COALESCE(NULLIF(l.sort_name, ''), lower(l.name)) AS sort_key, "
    });
    page.push(CARD).push(" AS card, ");
    // Name and brand weigh most, then the SKU, the category, the features.
    page.push(if fts.is_some() {
        "bm25(catalog_listing_fts, 10.0, 6.0, 2.0, 8.0, 1.0) AS score"
    } else {
        "0 AS score"
    });
    push_matching(&mut page, query, fts, None);
    page.push(") m) WHERE place = 1 ORDER BY ");
    page.push(match query.sort {
        ListingSort::PriceAsc => "from_minor, sort_key, product_id",
        ListingSort::PriceDesc => "from_minor DESC, sort_key, product_id",
        _ => within,
    });
    page.push(" LIMIT ")
        .push_bind(query.limit)
        .push(" OFFSET ")
        .push_bind(query.offset);
    let rows: Vec<ListingRow> = page.build_query_as().fetch_all(db).await?;

    let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(DISTINCT ");
    count.push(CARD).push(")");
    push_matching(&mut count, query, fts, None);
    let total: i64 = count.build_query_scalar().fetch_one(db).await?;

    let mut brands =
        QueryBuilder::<Sqlite>::new("SELECT l.brand_slug, MIN(l.brand_name), COUNT(DISTINCT ");
    brands.push(CARD).push(")");
    push_matching(&mut brands, query, fts, Some(Filter::Brand));
    brands.push(" GROUP BY l.brand_slug ORDER BY MIN(l.brand_name) COLLATE NOCASE, l.brand_slug");
    let mut brands: Vec<(String, String, i64)> = brands.build_query_as().fetch_all(db).await?;
    // Alphabetical for people: SQLite would put `Éclair` after `Zalman`.
    brands.sort_by_cached_key(|(slug, name, _)| (timada_core::slug::sort_key(name), slug.clone()));

    let mut prices = QueryBuilder::<Sqlite>::new(if in_currency {
        "SELECT MIN(p.price_minor), MAX(p.price_minor)"
    } else {
        "SELECT MIN(l.price_minor), MAX(l.price_minor)"
    });
    push_matching(&mut prices, query, fts, Some(Filter::Price));
    let (cheapest, dearest): (Option<i64>, Option<i64>) =
        prices.build_query_as().fetch_one(db).await?;

    // A card counts once, whichever of its versions qualifies.
    let mut stock =
        QueryBuilder::<Sqlite>::new("SELECT COUNT(DISTINCT CASE WHEN l.available > 0 THEN ");
    stock.push(CARD).push(" END)");
    push_matching(&mut stock, query, fts, Some(Filter::Stock));
    let in_stock: i64 = stock.build_query_scalar().fetch_one(db).await?;

    let mut rated = QueryBuilder::<Sqlite>::new("SELECT ");
    for (nth, stars) in ["4", "3", "2", "1"].into_iter().enumerate() {
        rated
            .push(if nth == 0 { "" } else { ", " })
            .push("COUNT(DISTINCT CASE WHEN l.rating_avg >= ")
            .push(stars)
            .push(" THEN ")
            .push(CARD)
            .push(" END)");
    }
    push_matching(&mut rated, query, fts, Some(Filter::Rating));
    let (four, three, two, one): (i64, i64, i64, i64) =
        rated.build_query_as().fetch_one(db).await?;

    let mut specs = Vec::new();
    for key in &query.facet_specs {
        let mut values = QueryBuilder::<Sqlite>::new("SELECT s.value, COUNT(DISTINCT ");
        values.push(CARD).push(")");
        push_matching(&mut values, query, fts, Some(Filter::Spec(key)));
        // "24 pouces" before "27 pouces" before "100 Hz"… by their number;
        // words (a cast gives them 0) by the alphabet.
        values.push(" GROUP BY s.value ORDER BY CAST(s.value AS REAL), s.value COLLATE NOCASE");
        let values: Vec<(String, i64)> = values.build_query_as().fetch_all(db).await?;
        if !values.is_empty() {
            specs.push(SpecFacet {
                key: key.clone(),
                values,
            });
        }
    }

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
            specs,
        },
    })
}

/// Gives the rows written before names had a sort key theirs, and returns how
/// many. Until then such a row is sorted by its lower-cased name; every row
/// gets its key anyway the next time anything about its product changes.
/// Cheap, safe to run at every start.
pub async fn fill_listing_sort_names(db: &SqlitePool) -> sqlx::Result<u64> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT product_id, name FROM catalog_listing WHERE sort_name = ''")
            .fetch_all(db)
            .await?;
    let mut filled = 0;
    for (product_id, name) in rows {
        filled += sqlx::query("UPDATE catalog_listing SET sort_name = ? WHERE product_id = ?")
            .bind(timada_core::slug::sort_key(&name))
            .bind(product_id)
            .execute(db)
            .await?
            .rows_affected();
    }
    Ok(filled)
}

/// How many cards — products on sale, a family counting once — each of
/// `category_ids` spans: its own and those of the categories under it.
/// Categories without any are left out.
///
/// With a `currency`, only what is sold in it counts.
pub async fn listed_counts_by_category(
    db: &SqlitePool,
    category_ids: &[String],
    currency: Option<&str>,
) -> sqlx::Result<std::collections::HashMap<String, i64>> {
    let mut counts = std::collections::HashMap::new();
    for category_id in category_ids {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT COALESCE(l.family_id, l.product_id)) FROM catalog_listing l
             WHERE l.archived = 0 AND l.price_minor IS NOT NULL
               AND instr(l.category_trail, ?1) > 0
               AND (?2 IS NULL OR EXISTS (
                    SELECT 1 FROM catalog_listing_currency_price p
                    WHERE p.product_id = l.product_id AND p.currency = ?2))",
        )
        .bind(format!("/{category_id}/"))
        .bind(currency)
        .fetch_one(db)
        .await?;
        if count > 0 {
            counts.insert(category_id.clone(), count);
        }
    }
    Ok(counts)
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

/// Gives every version of a family the rating of all their published reviews
/// together.
async fn refresh_family_rating(db: &SqlitePool, family_id: &str) -> sqlx::Result<()> {
    let (rating_avg, review_count): (Option<f64>, i64) = sqlx::query_as(
        "SELECT AVG(r.rating), COUNT(*) FROM catalog_listing_review r
         JOIN catalog_listing l ON l.product_id = r.product_id
         WHERE l.family_id = ?",
    )
    .bind(family_id)
    .fetch_one(db)
    .await?;
    sqlx::query("UPDATE catalog_listing SET rating_avg = ?, review_count = ? WHERE family_id = ?")
        .bind(rating_avg)
        .bind(review_count)
        .bind(family_id)
        .execute(db)
        .await?;
    Ok(())
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
    let priced = load_product_price(executor, price_id(product_id))
        .await?
        .filter(|price| !price.withdrawn);
    // Every currency the product is sold in, the listed one first.
    let prices: Vec<timada_core::Money> = priced
        .iter()
        .flat_map(|price| {
            std::iter::once(price.price_incl_tax.clone()).chain(price.currency_prices.clone())
        })
        .collect();
    let price = priced.map(|price| price.price_incl_tax);
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
    // Read from the event store: the family list may trail behind.
    let family = match &product.family_id {
        Some(family_id) => Command(executor).load_family(family_id).await?,
        None => None,
    };
    let thumbnail = product
        .media
        .iter()
        .find(|media| media.kind == MediaKind::Image);
    // The family the row was in: its rating changes when a version leaves.
    let family_before: Option<String> =
        sqlx::query_scalar("SELECT family_id FROM catalog_listing WHERE product_id = ?")
            .bind(product_id)
            .fetch_optional(db)
            .await?
            .flatten();

    let rowid: i64 = sqlx::query_scalar(
        "INSERT INTO catalog_listing
            (product_id, sku, name, sort_name, brand_name, brand_slug, category_id, category_trail,
             short_description, thumbnail_url, thumbnail_alt, price_minor, currency, available,
             rating_avg, review_count, archived, created_at, updated_at, family_id, family_name)
         VALUES (?1, ?2, ?3, ?18, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17,
                 ?19, ?20)
         ON CONFLICT (product_id) DO UPDATE
         SET name = excluded.name, sort_name = excluded.sort_name,
             brand_name = excluded.brand_name,
             brand_slug = excluded.brand_slug, category_id = excluded.category_id,
             category_trail = excluded.category_trail,
             short_description = excluded.short_description,
             thumbnail_url = excluded.thumbnail_url, thumbnail_alt = excluded.thumbnail_alt,
             price_minor = excluded.price_minor, currency = excluded.currency,
             available = excluded.available, rating_avg = excluded.rating_avg,
             review_count = excluded.review_count, archived = excluded.archived,
             family_id = excluded.family_id, family_name = excluded.family_name,
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
    .bind(timada_core::slug::sort_key(&product.name))
    .bind(family.as_ref().map(|family| family.id.as_str()))
    .bind(family.as_ref().map(|family| family.name.as_str()))
    .fetch_one(db)
    .await?;

    // The row was written with the product's own rating; in a family, the
    // rating is the family's — and that of the one it left changes too.
    let family_now = family.as_ref().map(|family| family.id.as_str());
    if let Some(family_id) = family_now {
        refresh_family_rating(db, family_id).await?;
    }
    if let Some(left) = family_before
        .as_deref()
        .filter(|id| Some(*id) != family_now)
    {
        refresh_family_rating(db, left).await?;
    }

    // The prices, rewritten whole: a currency the product left is gone.
    sqlx::query("DELETE FROM catalog_listing_currency_price WHERE product_id = ?")
        .bind(&product.id)
        .execute(db)
        .await?;
    for price in &prices {
        sqlx::query(
            "INSERT OR REPLACE INTO catalog_listing_currency_price (product_id, currency, price_minor)
             VALUES (?, ?, ?)",
        )
        .bind(&product.id)
        .bind(&price.currency)
        .bind(price.minor)
        .execute(db)
        .await?;
    }

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

    // The technical sheet, line by line. A label given twice in a group keeps
    // its last value.
    sqlx::query("DELETE FROM catalog_listing_spec WHERE product_id = ?")
        .bind(&product.id)
        .execute(db)
        .await?;
    for spec in &product.specs {
        let value = spec.value.trim();
        if spec.label.trim().is_empty() || value.is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO catalog_listing_spec (product_id, spec_group, label, value)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (product_id, spec_group, label) DO UPDATE SET value = excluded.value",
        )
        .bind(&product.id)
        .bind(spec.group.trim())
        .bind(spec.label.trim())
        .bind(value)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// The specs the products on sale under a category have, most common first,
/// with how many products have each: what an operator picks filters from.
pub async fn specs_in_category(
    db: &SqlitePool,
    category_id: &str,
) -> sqlx::Result<Vec<(SpecKey, i64)>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT s.spec_group, s.label, COUNT(*) AS products
         FROM catalog_listing_spec s JOIN catalog_listing l ON l.product_id = s.product_id
         WHERE l.archived = 0 AND l.price_minor IS NOT NULL AND instr(l.category_trail, ?) > 0
         GROUP BY s.spec_group, s.label
         ORDER BY products DESC, s.spec_group COLLATE NOCASE, s.label COLLATE NOCASE",
    )
    .bind(format!("/{category_id}/"))
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(group, label, products)| (SpecKey::new(group, label), products))
        .collect())
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
async fn on_product_specified<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductSpecified>,
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
async fn on_product_joined_family<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductJoinedFamily>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_product_left_family<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductLeftFamily>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

/// The name its cards go by — the one the family has *now*, so a redelivered
/// rename does not bring an old one back.
#[evento::subscription]
async fn on_family_renamed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<FamilyRenamed>,
) -> anyhow::Result<()> {
    let Some(family) = Command(ctx.executor)
        .load_family(&event.aggregate_id)
        .await?
    else {
        return Ok(());
    };
    sqlx::query("UPDATE catalog_listing SET family_name = ? WHERE family_id = ?")
        .bind(&family.name)
        .bind(&family.id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
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
async fn on_currency_price_set<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CurrencyPriceSet>,
) -> anyhow::Result<()> {
    refresh_priced(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn on_currency_price_removed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CurrencyPriceRemoved>,
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
