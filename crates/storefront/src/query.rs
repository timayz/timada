//! What a storefront page reads: the published products of the local copy.

use timada_catalog::{TreeNode, TreeOf, tree_of};
use timada_core::db::{Db, FromRow, Json, Row};

use crate::money::MoneyFormat;

/// A picture of a product. `url` is where a browser loads it from: absolute
/// once the host set [`StoreConfig::media_url`](crate::StoreConfig::media_url),
/// and the bare file name otherwise.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct Image {
    pub id: String,
    /// `<id>.<ext>`.
    pub file: String,
    pub mime: String,
    /// What the picture shows; may be empty.
    pub alt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Filled in from the store's media address when the product is read.
    #[serde(default)]
    pub url: String,
}

/// A way a product varies, with the values it takes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct ProductOption {
    pub id: String,
    pub name: String,
    pub values: Vec<String>,
}

/// One combination a shopper buys, with its own price.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct Variant {
    pub id: String,
    /// A value for every option of the product, in the options' order.
    pub selections: Vec<Selection>,
    /// The merchant's own code for it.
    pub sku: Option<String>,
    pub amount_cents: i64,
    pub currency: String,
    /// One of the product's pictures, shown when this variant is picked.
    pub image_id: Option<String>,
}

impl Variant {
    /// The price as a person reads it (`22.90 €`).
    pub fn price(&self) -> String {
        self.price_with(&MoneyFormat::default())
    }

    /// The price as `money` writes it ([`money`](crate::money)).
    pub fn price_with(&self, money: &MoneyFormat) -> String {
        money.format(self.amount_cents.max(0) as u64, &self.currency)
    }

    /// The value it takes for an option, if it names one.
    pub fn value_of(&self, option_id: &str) -> Option<&str> {
        self.selections
            .iter()
            .find(|s| s.option_id == option_id)
            .map(|s| s.value.as_str())
    }

    /// Whether this is the variant those `(option id, value)` pairs name,
    /// whatever order they come in.
    pub fn matches(&self, picked: &[(String, String)]) -> bool {
        self.selections.len() == picked.len()
            && self.selections.iter().all(|mine| {
                picked
                    .iter()
                    .any(|(option_id, value)| *option_id == mine.option_id && *value == mine.value)
            })
    }
}

/// One option of a variant and the value it takes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct Selection {
    pub option_id: String,
    pub value: String,
}

/// A fact in a product's table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

/// What a shopper may pay for a product: the two amounts are equal when
/// nothing varies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PriceRange {
    pub min_cents: i64,
    pub max_cents: i64,
    pub currency: String,
}

impl PriceRange {
    /// Whether the price depends on what a shopper picks.
    pub fn varies(&self) -> bool {
        self.min_cents != self.max_cents
    }

    /// `12.50 €`, or `12.00–18.00 €` when it varies.
    pub fn label(&self) -> String {
        self.label_with(&MoneyFormat::default())
    }

    /// [`label`](Self::label) as `money` writes amounts
    /// ([`money`](crate::money)): `12,50€`, or `12,00€–18,50€` when it
    /// varies (the currency is dropped from the first amount only when it
    /// ends it).
    pub fn label_with(&self, money: &MoneyFormat) -> String {
        let max = money.format(self.max_cents.max(0) as u64, &self.currency);
        if !self.varies() {
            return max;
        }
        money.range(
            self.min_cents.max(0) as u64,
            self.max_cents.max(0) as u64,
            &self.currency,
        )
    }
}

/// A product as the storefront shows it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Product {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    /// Unix seconds of the last change.
    pub updated_at: i64,
    /// Its pictures, in the order the merchant put them.
    pub images: Vec<Image>,
    /// The ways it varies; empty when it does not.
    pub options: Vec<ProductOption>,
    /// The combinations it sells, each with its own price.
    pub variants: Vec<Variant>,
    /// The facts in its table.
    pub attributes: Vec<Attribute>,
    pub min_amount_cents: Option<i64>,
    pub max_amount_cents: Option<i64>,
    /// How many published reviews it has.
    pub review_count: i64,
    /// Their average times a hundred (`435` is 4.35 stars); 0 without any.
    pub rating_x100: i64,
    /// What is on the shelf of each thing the shop counts: a variant's id,
    /// or `""` for a product without variants. A thing missing here sells
    /// without limit.
    pub stock: std::collections::BTreeMap<String, u32>,
    /// Whether anything of it can still be bought.
    pub in_stock: bool,
}

impl FromRow for Product {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            slug: row.get("slug")?,
            name: row.get("name")?,
            description: row.get("description")?,
            amount_cents: row.get("amount_cents")?,
            currency: row.get("currency")?,
            updated_at: row.get("updated_at")?,
            images: row.get::<Json<_>>("images_json")?.0,
            options: row.get::<Json<_>>("options_json")?.0,
            variants: row.get::<Json<_>>("variants_json")?.0,
            attributes: row.get::<Json<_>>("attributes_json")?.0,
            min_amount_cents: row.get("min_amount_cents")?,
            max_amount_cents: row.get("max_amount_cents")?,
            review_count: row.get("review_count")?,
            rating_x100: row.get("rating_x100")?,
            stock: row.get::<Json<_>>("stock_json")?.0,
            in_stock: row.get("in_stock")?,
        })
    }
}

impl Product {
    /// How many are left of the product (`variant_id` `None`) or of one of
    /// its variants; `None` when the shop does not count it (no limit).
    pub fn left(&self, variant_id: Option<&str>) -> Option<u32> {
        self.stock.get(variant_id.unwrap_or("")).copied()
    }

    /// Whether that product or variant is sold out.
    pub fn is_sold_out(&self, variant_id: Option<&str>) -> bool {
        self.left(variant_id) == Some(0)
    }

    /// The average of its published reviews, to one decimal (`4.4`); `None`
    /// before the first one.
    pub fn rating(&self) -> Option<f64> {
        (self.review_count > 0).then(|| (self.rating_x100 as f64 / 10.0).round() / 10.0)
    }

    /// Its average as whole stars, rounded (`4` for 4.4): what a row of star
    /// glyphs shows.
    pub fn rating_stars(&self) -> Option<u8> {
        (self.review_count > 0).then(|| ((self.rating_x100 + 50) / 100).clamp(1, 5) as u8)
    }

    /// The price as a listing shows it: `12.50 €`, or `12.00–18.00 €`
    /// when the variants differ. Empty when it has no price at all.
    pub fn price(&self) -> Option<String> {
        Some(self.price_range()?.label())
    }

    /// [`price`](Self::price) as `money` writes amounts
    /// ([`money`](crate::money)): what the [`price`](crate::price) component
    /// shows.
    pub fn price_with(&self, money: &MoneyFormat) -> Option<String> {
        Some(self.price_range()?.label_with(money))
    }

    /// The cheapest and the dearest a shopper may pay.
    pub fn price_range(&self) -> Option<PriceRange> {
        Some(PriceRange {
            min_cents: self.min_amount_cents?,
            max_cents: self.max_amount_cents?,
            currency: self.currency.clone()?,
        })
    }

    /// The variant those `(option id, value)` pairs name, if the product
    /// sells it: what a page resolves its menus to.
    pub fn variant(&self, picked: &[(String, String)]) -> Option<&Variant> {
        self.variants.iter().find(|variant| variant.matches(picked))
    }

    /// The picture a listing shows, when it has one.
    pub fn image(&self) -> Option<&Image> {
        self.images.first()
    }

    /// Points every picture at `base`, the address the store's pictures are
    /// served from; without one they keep their bare file name, which a host
    /// serving them itself can still use.
    ///
    /// [`products`](crate::products) and the other context helpers do this
    /// already. It is public for a host that reads the rows itself with
    /// [`published_products`] and friends.
    pub fn fill_urls(&mut self, base: Option<&str>) {
        for image in &mut self.images {
            image.url = match base {
                Some(base) => format!("{base}/{}", image.file),
                None => image.file.clone(),
            };
        }
    }
}

/// How many products a page shows unless asked otherwise.
pub const DEFAULT_PER_PAGE: u32 = 24;
pub const MAX_PER_PAGE: u32 = 100;

/// How a page of products is ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ProductSort {
    /// Most recently changed first.
    #[default]
    Newest,
    /// Cheapest first; a product with no price comes last.
    PriceAsc,
    /// Most expensive first; a product with no price comes last.
    PriceDesc,
    /// Best average first, the most reviewed first among equals; a product
    /// with no published review comes last.
    TopRated,
}

impl ProductSort {
    /// The value a sort `<select>` and a query string use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::PriceAsc => "price_asc",
            Self::PriceDesc => "price_desc",
            Self::TopRated => "top_rated",
        }
    }

    /// The sort a query string asks for, or [`Newest`](Self::Newest) for
    /// anything else.
    pub fn parse(value: &str) -> Self {
        match value {
            "price_asc" => Self::PriceAsc,
            "price_desc" => Self::PriceDesc,
            "top_rated" => Self::TopRated,
            _ => Self::Newest,
        }
    }
}

/// A page of the catalog: 1-based `page`, `per_page` capped at
/// [`MAX_PER_PAGE`], an optional search over name and slug, a category whose
/// page it is, the categories a product must be in (any of them), the products
/// to leave out and the order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductQuery {
    pub page: u32,
    pub per_page: u32,
    pub q: Option<String>,
    /// A category whose page this is: the products in it and in everything
    /// below it.
    pub category: Option<String>,
    /// Products filed directly under any of these; empty means every
    /// category.
    pub categories: Vec<String>,
    /// Products to leave out, by id: what "related products" needs.
    pub exclude: Vec<String>,
    /// The least a product may cost, in minor units: one whose dearest
    /// price is under it is out.
    pub min_cents: Option<i64>,
    /// The most a product may cost: one whose cheapest price is over it is
    /// out.
    pub max_cents: Option<i64>,
    /// `(option key, value key)` pairs a product must every one of, as
    /// [`facet_key`] writes them: "Colour: Rose".
    pub options: Vec<(String, String)>,
    pub sort: ProductSort,
}

impl Default for ProductQuery {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: DEFAULT_PER_PAGE,
            q: None,
            category: None,
            categories: Vec::new(),
            exclude: Vec::new(),
            min_cents: None,
            max_cents: None,
            options: Vec::new(),
            sort: ProductSort::default(),
        }
    }
}

impl ProductQuery {
    pub fn page(mut self, page: u32) -> Self {
        self.page = page.max(1);
        self
    }

    pub fn per_page(mut self, per_page: u32) -> Self {
        self.per_page = per_page.clamp(1, MAX_PER_PAGE);
        self
    }

    pub fn search(mut self, q: impl Into<String>) -> Self {
        let q = q.into();
        self.q = (!q.trim().is_empty()).then(|| q.trim().to_owned());
        self
    }

    /// Only what the page of this category lists: the products in it and in
    /// every category below it.
    pub fn in_category(mut self, id: impl Into<String>) -> Self {
        self.category = Some(id.into());
        self
    }

    /// Only products filed directly under one of these categories.
    pub fn in_categories(mut self, ids: impl IntoIterator<Item = String>) -> Self {
        self.categories = ids.into_iter().collect();
        self
    }

    pub fn exclude(mut self, id: impl Into<String>) -> Self {
        self.exclude.push(id.into());
        self
    }

    /// Leaves out every one of these products too.
    pub fn exclude_all(mut self, ids: impl IntoIterator<Item = String>) -> Self {
        self.exclude.extend(ids);
        self
    }

    /// Only products a shopper could pay between `min` and `max` for, in
    /// minor units, either bound optional: a product is in when its price
    /// range overlaps the band, so one sold from 9.90 to 59.90 shows under
    /// "up to 50" and under "50 to 70" alike. One with no price never matches
    /// a band.
    pub fn priced_between(mut self, min: Option<i64>, max: Option<i64>) -> Self {
        self.min_cents = min;
        self.max_cents = max;
        self
    }

    /// Only products whose option `name` takes `value`, both compared the way
    /// [`facet_key`] folds them. Several calls narrow further (every pair
    /// must hold).
    pub fn with_option(mut self, name: &str, value: &str) -> Self {
        self.options.push((facet_key(name), facet_key(value)));
        self
    }

    pub fn sort(mut self, sort: ProductSort) -> Self {
        self.sort = sort;
        self
    }
}

/// One value an option takes across the products of a listing, and how many
/// of them take it: what a filter's links are made of.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FacetValue {
    /// What a link carries, as [`facet_key`] writes it: `rose-fonce`.
    pub key: String,
    /// The value as a merchant wrote it: `Rose foncé`. When products spell
    /// it differently, one of the spellings.
    pub label: String,
    pub count: i64,
}

/// The products of one page and what is around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductPage {
    pub items: Vec<Product>,
    pub page: u32,
    pub per_page: u32,
    /// How many published products match, over every page.
    pub total: i64,
}

impl ProductPage {
    pub fn pages(&self) -> u32 {
        let per_page = i64::from(self.per_page.max(1));
        ((self.total + per_page - 1) / per_page).max(1) as u32
    }

    pub fn has_next(&self) -> bool {
        self.page < self.pages()
    }

    pub fn has_prev(&self) -> bool {
        self.page > 1
    }
}

use timada_core::db::fts_query as phrase;
use timada_core::like_contains as contains;

/// Where a page of products comes from, `?1` being what was searched for.
/// With a phrase ([`phrase`]: `q` long enough for the index, as one folded
/// phrase) the full-text index over `slug_search` and `name_search` narrows
/// the products first, so a search costs what it matches. Without one, `?1`
/// is a `LIKE` pattern or `NULL`.
macro_rules! products_from {
    (phrase) => {
        "FROM storefront_products p
         WHERE p.id IN (SELECT id FROM storefront_products
                        WHERE fts_match(slug_search, name_search, ?1)) "
    };
    (pattern) => {
        "FROM storefront_products p
         WHERE (?1 IS NULL OR p.slug LIKE ?1 ESCAPE '\\' OR p.name LIKE ?1 ESCAPE '\\') "
    };
}

/// What a page of products is filtered by besides the search, shared by the
/// count, the list and the facets so they never disagree. `?2` is the JSON
/// array of category ids (`[]` for every category), `?3` the JSON ids to leave
/// out, `?4` a category whose page it is (read from what the subscription
/// stored for it, so a sub-category costs nothing more), `?7` and `?8` the
/// price band a product's own range must overlap, `?9` the JSON array of
/// `[option key, value key]` pairs it must every one of (`[]` for none).
macro_rules! products_where {
    () => {
        "AND p.status = 'published'
           AND (?2 = '[]' OR p.id IN (SELECT product_id FROM storefront_product_categories
                WHERE category_id IN (SELECT value FROM json_each(?2))))
           AND (?3 = '[]' OR p.id NOT IN (SELECT value FROM json_each(?3)))
           AND (?4 IS NULL OR p.id IN (SELECT product_id FROM storefront_category_products
                WHERE category_id = ?4))
           AND (?7 IS NULL OR p.max_amount_cents >= ?7)
           AND (?8 IS NULL OR p.min_amount_cents <= ?8)
           AND (?9 = '[]' OR (SELECT count(*) FROM json_each(?9) o
                WHERE EXISTS (SELECT 1 FROM storefront_product_options po
                    WHERE po.product_id = p.id
                      AND po.name_key = json_extract(o.value, '$[0]')
                      AND po.value_key = json_extract(o.value, '$[1]')))
                = json_array_length(?9)) "
    };
}

/// One page of the list, ordered by `$order`: each order is its own statement
/// over the same filter, so every statement stays a literal.
macro_rules! products_page {
    ($from:ident, $order:literal) => {
        concat!(
            "SELECT p.id, p.slug, p.name, p.description, p.amount_cents, p.currency,
                p.updated_at, p.images_json, p.options_json, p.variants_json,
                p.attributes_json, p.min_amount_cents, p.max_amount_cents, p.review_count,
                p.rating_x100, p.stock_json, p.in_stock ",
            products_from!($from),
            products_where!(),
            $order,
            " LIMIT ?5 OFFSET ?6"
        )
    };
}

macro_rules! products_count {
    ($from:ident) => {
        concat!("SELECT count(*) ", products_from!($from), products_where!())
    };
}

/// Newest first, and by what a shopper would actually pay: the cheapest a
/// product can be going up, the dearest going down. One with no price sorts
/// last either way.
macro_rules! products_pages {
    ($from:ident) => {
        [
            products_page!($from, "ORDER BY p.updated_at DESC, p.id DESC"),
            products_page!(
                $from,
                "ORDER BY p.min_amount_cents IS NULL, p.min_amount_cents ASC, p.id DESC"
            ),
            products_page!(
                $from,
                "ORDER BY p.max_amount_cents IS NULL, p.max_amount_cents DESC, p.id DESC"
            ),
            products_page!(
                $from,
                "ORDER BY p.review_count = 0, p.rating_x100 DESC, p.review_count DESC, p.id DESC"
            ),
        ]
    };
}

const PAGES: [&str; 4] = products_pages!(pattern);
const COUNT: &str = products_count!(pattern);
const SEARCH_PAGES: [&str; 4] = products_pages!(phrase);
const SEARCH_COUNT: &str = products_count!(phrase);

/// The values option `?10` takes among the products the filter keeps, each
/// with its count, in key order; one spelling per key.
macro_rules! products_facet {
    ($from:ident) => {
        concat!(
            "SELECT po.value_key, min(po.value), count(*)
             FROM storefront_product_options po
             WHERE po.name_key = ?10
               AND po.product_id IN (SELECT p.id ",
            products_from!($from),
            products_where!(),
            ") GROUP BY po.value_key ORDER BY po.value_key"
        )
    };
}

const FACET: &str = products_facet!(pattern);
const SEARCH_FACET: &str = products_facet!(phrase);

/// The `?9` bind: the pairs as a JSON array of two-element arrays.
fn options_json(options: &[(String, String)]) -> String {
    serde_json::to_string(options).unwrap_or_else(|_| "[]".to_owned())
}

impl ProductSort {
    /// Its statement among the four of [`products_pages`].
    fn index(self) -> usize {
        match self {
            ProductSort::Newest => 0,
            ProductSort::PriceAsc => 1,
            ProductSort::PriceDesc => 2,
            ProductSort::TopRated => 3,
        }
    }
}

/// One page of a category, read off what the subscription keeps for it: the
/// rows are already in `$order` in an index of their own, so the page costs
/// its own size however large the store is. `?1` is the category, `?2` and
/// `?3` the limit and offset.
macro_rules! category_page {
    ($order:literal) => {
        concat!(
            "SELECT p.id, p.slug, p.name, p.description, p.amount_cents, p.currency,
                p.updated_at, p.images_json, p.options_json, p.variants_json,
                p.attributes_json, p.min_amount_cents, p.max_amount_cents, p.review_count,
                p.rating_x100, p.stock_json, p.in_stock
             FROM storefront_category_products cp
             JOIN storefront_products p ON p.id = cp.product_id
             WHERE cp.category_id = ?1 ",
            $order,
            " LIMIT ?2 OFFSET ?3"
        )
    };
}

const CATEGORY_NEWEST: &str = category_page!("ORDER BY cp.updated_at DESC, cp.product_id DESC");
const CATEGORY_PRICE_ASC: &str = category_page!(
    "ORDER BY cp.min_amount_cents IS NULL, cp.min_amount_cents ASC, cp.product_id DESC"
);
const CATEGORY_PRICE_DESC: &str = category_page!(
    "ORDER BY cp.max_amount_cents IS NULL, cp.max_amount_cents DESC, cp.product_id DESC"
);
const CATEGORY_TOP_RATED: &str = category_page!(
    "ORDER BY cp.review_count = 0, cp.rating_x100 DESC, cp.review_count DESC, cp.product_id DESC"
);

/// The published products a query matches, in the order it asks for.
pub async fn published_products(db: &Db, query: &ProductQuery) -> anyhow::Result<ProductPage> {
    let per_page = i64::from(query.per_page.clamp(1, MAX_PER_PAGE));
    let page = query.page.max(1);
    // A search long enough for the index reads it; a shorter one is a
    // `LIKE` over the rows.
    let phrase = query.q.as_deref().and_then(phrase);
    let (pages, count) = if phrase.is_some() {
        (SEARCH_PAGES, SEARCH_COUNT)
    } else {
        (PAGES, COUNT)
    };
    let search = phrase.or_else(|| query.q.as_deref().map(contains));
    // A JSON array, read back by `json_each`: one bind for any number of ids.
    let categories = serde_json::to_string(&query.categories).unwrap_or_else(|_| "[]".to_owned());
    let exclude = serde_json::to_string(&query.exclude).unwrap_or_else(|_| "[]".to_owned());
    let options = options_json(&query.options);
    let offset = i64::from(page - 1) * per_page;
    // A category's page as it is, with nothing else to match: the count and
    // the order the subscription stored.
    if let Some(category) = query.category.as_deref()
        && search.is_none()
        && query.categories.is_empty()
        && query.exclude.is_empty()
        && query.min_cents.is_none()
        && query.max_cents.is_none()
        && query.options.is_empty()
    {
        let total: Option<(i64,)> = db
            .fetch_optional(
                "SELECT subtree_count FROM storefront_categories WHERE id = ?1",
                (category,),
            )
            .await?;
        let total = total.map_or(0, |(count,)| count);
        let statement = match query.sort {
            ProductSort::Newest => CATEGORY_NEWEST,
            ProductSort::PriceAsc => CATEGORY_PRICE_ASC,
            ProductSort::PriceDesc => CATEGORY_PRICE_DESC,
            ProductSort::TopRated => CATEGORY_TOP_RATED,
        };
        let items = db
            .fetch_all(statement, (category, per_page, offset))
            .await?;
        return Ok(ProductPage {
            items,
            page,
            per_page: per_page as u32,
            total,
        });
    }
    let params = || {
        (
            search.as_deref(),
            categories.as_str(),
            exclude.as_str(),
            query.category.as_deref(),
            per_page,
            offset,
            query.min_cents,
            query.max_cents,
            options.as_str(),
        )
    };
    let total: i64 = db.fetch_scalar(count, params()).await?;
    let items = db.fetch_all(pages[query.sort.index()], params()).await?;
    Ok(ProductPage {
        items,
        page,
        per_page: per_page as u32,
        total,
    })
}

/// The values `option` takes among the published products `query` matches,
/// each with how many take it, in key order: the links of a filter. The
/// query's own pairs on `option` are set aside, so the listing filtered to
/// "Colour: Rose" still offers the other colours; its band and its other
/// options hold.
pub async fn published_option_values(
    db: &Db,
    query: &ProductQuery,
    option: &str,
) -> anyhow::Result<Vec<FacetValue>> {
    let name_key = facet_key(option);
    let phrase = query.q.as_deref().and_then(phrase);
    let statement = if phrase.is_some() {
        SEARCH_FACET
    } else {
        FACET
    };
    let search = phrase.or_else(|| query.q.as_deref().map(contains));
    let categories = serde_json::to_string(&query.categories).unwrap_or_else(|_| "[]".to_owned());
    let exclude = serde_json::to_string(&query.exclude).unwrap_or_else(|_| "[]".to_owned());
    let others: Vec<(String, String)> = query
        .options
        .iter()
        .filter(|(name, _)| *name != name_key)
        .cloned()
        .collect();
    let rows: Vec<(String, String, i64)> = db
        .fetch_all(
            statement,
            (
                search.as_deref(),
                categories.as_str(),
                exclude.as_str(),
                query.category.as_deref(),
                // `?5` and `?6` are the page's limit and offset, which a facet
                // has no use for; every number up to the last one still needs
                // a value.
                0i64,
                0i64,
                query.min_cents,
                query.max_cents,
                options_json(&others),
                name_key.as_str(),
            ),
        )
        .await?;
    Ok(rows
        .into_iter()
        .map(|(key, label, count)| FacetValue { key, label, count })
        .collect())
}

/// What goes well with a cart holding `product_ids`: the published products
/// sharing a category with any of them, newest first, none of them already
/// in it. With no category to go by, the newest products.
pub async fn published_cart_recommendations(
    db: &Db,
    product_ids: &[String],
    limit: u32,
) -> anyhow::Result<Vec<Product>> {
    let ids = serde_json::to_string(product_ids).unwrap_or_else(|_| "[]".to_owned());
    let categories: Vec<(String,)> = db
        .fetch_all(
            "SELECT DISTINCT category_id FROM storefront_product_categories
             WHERE product_id IN (SELECT value FROM json_each(?1))",
            (ids,),
        )
        .await?;
    let query = ProductQuery::default()
        .per_page(limit)
        .in_categories(categories.into_iter().map(|(id,)| id))
        .exclude_all(product_ids.iter().cloned());
    Ok(published_products(db, &query).await?.items)
}

/// One published product, by slug.
pub async fn published_product_by_slug(db: &Db, slug: &str) -> anyhow::Result<Option<Product>> {
    db.fetch_optional(
        "SELECT id, slug, name, description, amount_cents, currency, updated_at,
            images_json, options_json, variants_json, attributes_json,
            min_amount_cents, max_amount_cents, review_count, rating_x100, stock_json, in_stock
         FROM storefront_products WHERE slug = ?1 AND status = 'published'",
        (slug,),
    )
    .await
}

/// The published products among `ids`, in no particular order: what a cart
/// page shows its lines with (a product missing here is off sale).
pub async fn published_products_by_ids(db: &Db, ids: &[&str]) -> anyhow::Result<Vec<Product>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // One placeholder per id rather than a `json_each` list: given the list
    // Turso walks every published product through the status index
    // (`SEARCH p USING INDEX storefront_products_price (status=?)`), so a
    // cart cost the whole catalogue; given the ids it reads them one by one
    // through the primary key (see the `probe` tests).
    let statement = format!(
        "SELECT id, slug, name, description, amount_cents, currency, updated_at,
            images_json, options_json, variants_json, attributes_json,
            min_amount_cents, max_amount_cents, review_count, rating_x100, stock_json, in_stock
         FROM storefront_products
         WHERE id IN ({}) AND status = 'published'",
        timada_core::db::placeholders(ids.len())
    );
    // Owned before the await: a parameter iterator borrowing `&[&str]`
    // carries two lifetimes into the future, and the pages that call this
    // through `cart` then fail to prove the future `Send` for every one of
    // them ("implementation of `Send` is not general enough").
    let params: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    db.fetch_all(&statement, params).await
}

// =============================================================================
// Categories
// =============================================================================

/// A category of the store, as a page shows it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq, Hash)]
pub struct Category {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    /// `None` for a top-level category.
    pub parent_id: Option<String>,
    /// Where it sorts among its siblings, lowest first.
    pub position: i64,
    /// Shown in the shop's menu. A hidden category keeps its page; it and
    /// everything under it are only left out of [`menu_of`].
    pub in_menu: bool,
}

impl TreeNode for Category {
    fn node_id(&self) -> &str {
        &self.id
    }

    fn node_parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }
}

/// A category and what hangs under it, with how many published products are
/// in it and its sub-categories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryNode {
    pub category: Category,
    pub children: Vec<CategoryNode>,
    /// Distinct published products this category's page lists: those in it
    /// and in every category below it, each counted once.
    pub product_count: i64,
}

impl CategoryNode {
    /// The categories from this one down to `id`, excluding `id` itself, if
    /// `id` is in this subtree.
    fn path_to(&self, id: &str) -> Option<Vec<Category>> {
        if self.category.id == id {
            return Some(Vec::new());
        }
        self.children.iter().find_map(|child| {
            let mut path = child.path_to(id)?;
            path.insert(0, self.category.clone());
            Some(path)
        })
    }

    /// The node of `slug` in this subtree, if any.
    pub fn find(&self, slug: &str) -> Option<&CategoryNode> {
        if self.category.slug == slug {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(slug))
    }
}

/// The node of `slug` among these trees, if any.
pub fn find_category<'a>(nodes: &'a [CategoryNode], slug: &str) -> Option<&'a CategoryNode> {
    nodes.iter().find_map(|node| node.find(slug))
}

/// The trees a menu shows: `nodes` without the categories the merchant hid
/// from it, each taking its subtree along.
pub fn menu_of(nodes: &[CategoryNode]) -> Vec<CategoryNode> {
    nodes
        .iter()
        .filter(|node| node.category.in_menu)
        .map(|node| CategoryNode {
            category: node.category.clone(),
            children: menu_of(&node.children),
            product_count: node.product_count,
        })
        .collect()
}

/// What a search compares: `text` without accents or case, its whitespace
/// collapsed, so "LITIERE" finds "Litière chat".
pub fn search_key(text: &str) -> String {
    deunicode::deunicode(text)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a filter carries and compares: [`search_key`] with its spaces as
/// dashes, so "Rose foncé" is `rose-fonce` in a link and in the rows alike.
pub fn facet_key(text: &str) -> String {
    search_key(text).replace(' ', "-")
}

/// The categories among `nodes` (and everything under them) whose name
/// contains `q`, accents and case aside: the ones whose name starts with it
/// first, then the rest, each group in tree order; at most `limit`. Nothing
/// for a blank `q`.
pub fn search_categories<'a>(
    nodes: &'a [CategoryNode],
    q: &str,
    limit: usize,
) -> Vec<&'a CategoryNode> {
    let q = search_key(q);
    if q.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut starting = Vec::new();
    let mut containing = Vec::new();
    fn walk<'a>(
        nodes: &'a [CategoryNode],
        q: &str,
        starting: &mut Vec<&'a CategoryNode>,
        containing: &mut Vec<&'a CategoryNode>,
    ) {
        for node in nodes {
            let name = search_key(&node.category.name);
            if name.starts_with(q) {
                starting.push(node);
            } else if name.contains(q) {
                containing.push(node);
            }
            walk(&node.children, q, starting, containing);
        }
    }
    walk(nodes, &q, &mut starting, &mut containing);
    starting.extend(containing);
    starting.truncate(limit);
    starting
}

/// A category row with the count its page lists, as the subscription stored
/// it.
struct CountedCategory {
    category: Category,
    subtree_count: i64,
}

impl FromRow for CountedCategory {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self> {
        Ok(Self {
            category: Category::from_row(row)?,
            subtree_count: row.get("subtree_count")?,
        })
    }
}

impl TreeNode for CountedCategory {
    fn node_id(&self) -> &str {
        &self.category.id
    }

    fn node_parent_id(&self) -> Option<&str> {
        self.category.parent_id.as_deref()
    }
}

/// The live categories of the store as trees, each level in its own order,
/// with how many published products each one's page lists.
pub async fn published_categories(db: &Db) -> anyhow::Result<Vec<CategoryNode>> {
    let categories: Vec<CountedCategory> = db
        .fetch_all(
            "SELECT id, slug, name, description, parent_id, position, in_menu, subtree_count
             FROM storefront_categories WHERE archived = 0
             ORDER BY position, name, id",
            (),
        )
        .await?;
    Ok(tree_of(categories).into_iter().map(node_of).collect())
}

/// Turns a tree of counted categories into one of [`CategoryNode`].
fn node_of(node: TreeOf<CountedCategory>) -> CategoryNode {
    CategoryNode {
        category: node.item.category,
        children: node.children.into_iter().map(node_of).collect(),
        product_count: node.item.subtree_count,
    }
}

/// One live category, by slug.
pub async fn published_category_by_slug(db: &Db, slug: &str) -> anyhow::Result<Option<Category>> {
    db.fetch_optional(
        "SELECT id, slug, name, description, parent_id, position, in_menu
         FROM storefront_categories WHERE slug = ?1 AND archived = 0",
        (slug,),
    )
    .await
}

/// The text a live category's page shows below its products, empty when
/// it has none. Apart from [`Category`], so the trees every page loads do
/// not carry it.
pub async fn published_category_body(db: &Db, category_id: &str) -> anyhow::Result<String> {
    let body: Option<(String,)> = db
        .fetch_optional(
            "SELECT body FROM storefront_categories WHERE id = ?1 AND archived = 0",
            (category_id,),
        )
        .await?;
    Ok(body.map(|(body,)| body).unwrap_or_default())
}

/// The live categories above `category`, top-level first: a breadcrumb,
/// read off the menu a page has already loaded. Stops where a parent is
/// archived, since the tree does.
pub fn category_ancestors(nodes: &[CategoryNode], category: &Category) -> Vec<Category> {
    nodes
        .iter()
        .find_map(|node| node.path_to(&category.id))
        .unwrap_or_default()
}

/// The live categories a product is filed under, in tree order.
pub async fn published_product_categories(
    db: &Db,
    product_id: &str,
) -> anyhow::Result<Vec<Category>> {
    db.fetch_all(
        "SELECT c.id, c.slug, c.name, c.description, c.parent_id, c.position, c.in_menu
         FROM storefront_categories c
         JOIN storefront_product_categories pc ON pc.category_id = c.id
         WHERE pc.product_id = ?1 AND c.archived = 0
         ORDER BY c.position, c.name, c.id",
        (product_id,),
    )
    .await
}

/// The published products sharing a category with `product`, newest first,
/// without `product` itself.
pub async fn published_related_products(
    db: &Db,
    product: &Product,
    limit: u32,
) -> anyhow::Result<Vec<Product>> {
    let categories: Vec<(String,)> = db
        .fetch_all(
            "SELECT category_id FROM storefront_product_categories WHERE product_id = ?1",
            (product.id.as_str(),),
        )
        .await?;
    if categories.is_empty() {
        return Ok(Vec::new());
    }
    let query = ProductQuery::default()
        .per_page(limit)
        .in_categories(categories.into_iter().map(|(id,)| id))
        .exclude(product.id.clone());
    Ok(published_products(db, &query).await?.items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, children: Vec<CategoryNode>) -> CategoryNode {
        CategoryNode {
            category: Category {
                id: name.to_owned(),
                slug: name.to_lowercase(),
                name: name.to_owned(),
                description: String::new(),
                parent_id: None,
                position: 0,
                in_menu: true,
            },
            children,
            product_count: 0,
        }
    }

    fn tree() -> Vec<CategoryNode> {
        vec![
            node("Couchage", vec![node("Petit lit", vec![])]),
            node("Litière chat", vec![node("Litière de voyage", vec![])]),
            node("Jouets", vec![]),
        ]
    }

    fn names<'a>(found: &[&'a CategoryNode]) -> Vec<&'a str> {
        found.iter().map(|n| n.category.name.as_str()).collect()
    }

    #[test]
    fn facet_key_dashes_the_spaces() {
        assert_eq!(facet_key("Rose foncé"), "rose-fonce");
        assert_eq!(facet_key("  Bleu   Foncé "), "bleu-fonce");
        assert_eq!(facet_key("36-37"), "36-37");
        assert_eq!(facet_key(""), "");
    }

    #[test]
    fn search_key_drops_accents_case_and_extra_spaces() {
        assert_eq!(search_key("  Litière   DE voyage "), "litiere de voyage");
        assert_eq!(search_key("Été"), "ete");
    }

    #[test]
    fn categories_matching_a_prefix_come_first_then_the_rest_in_tree_order() {
        let tree = tree();
        assert_eq!(
            names(&search_categories(&tree, "lit", 10)),
            ["Litière chat", "Litière de voyage", "Petit lit"]
        );
    }

    #[test]
    fn categories_match_without_accents_or_case() {
        let tree = tree();
        assert_eq!(
            names(&search_categories(&tree, "LITIERE DE", 10)),
            ["Litière de voyage"]
        );
    }

    #[test]
    fn category_search_honours_the_limit_and_ignores_a_blank_query() {
        let tree = tree();
        assert_eq!(
            names(&search_categories(&tree, "lit", 2)),
            ["Litière chat", "Litière de voyage"]
        );
        assert!(search_categories(&tree, "   ", 10).is_empty());
        assert!(search_categories(&tree, "lit", 0).is_empty());
        assert!(search_categories(&tree, "chien", 10).is_empty());
    }
}

/// The filters over a small catalog in an in-memory database.
#[cfg(test)]
mod sql {
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

    async fn insert_product(
        db: &Db,
        id: &str,
        status: &str,
        range: Option<(i64, i64)>,
        colours: &[&str],
    ) {
        let options = if colours.is_empty() {
            "[]".to_owned()
        } else {
            serde_json::to_string(&[ProductOption {
                id: "opt-colour".to_owned(),
                name: "Couleur".to_owned(),
                values: colours.iter().map(|c| (*c).to_owned()).collect(),
            }])
            .unwrap()
        };
        // What the subscription writes: the row, with what a search finds
        // it by.
        db.execute(
            "INSERT INTO storefront_products (id, slug, name, description, amount_cents,
                currency, status, updated_at, images_json, options_json, variants_json,
                attributes_json, min_amount_cents, max_amount_cents, slug_search, name_search)
             VALUES (?1, ?1, ?1, '', ?2, 'EUR', ?3, 1, '[]', ?4, '[]', '[]', ?5, ?6, ?7, ?7)",
            (
                id,
                range.map(|(min, _)| min),
                status,
                options.as_str(),
                range.map(|(min, _)| min),
                range.map(|(_, max)| max),
                timada_core::db::search_key(id),
            ),
        )
        .await
        .unwrap();
        for colour in colours {
            db.execute(
                crate::read_model::INSERT_OPTION,
                (id, "couleur", facet_key(colour), "Couleur", *colour),
            )
            .await
            .unwrap();
        }
    }

    /// A collar from 9.90 to 59.90 in rose or blue, a grey basket at 34.90,
    /// an unpriced service, and a draft nobody sees; the first two in a
    /// category whose stored count is 2.
    async fn catalog() -> Db {
        let db = db().await;
        insert_product(
            &db,
            "collar",
            "published",
            Some((990, 5990)),
            &["Rose", "Bleu"],
        )
        .await;
        insert_product(&db, "basket", "published", Some((3490, 3490)), &["Gris"]).await;
        insert_product(&db, "service", "published", None, &[]).await;
        insert_product(&db, "draft", "draft", Some((1000, 1000)), &["Rose"]).await;
        db.execute(
            "INSERT INTO storefront_categories (id, slug, name, description, body, position,
                archived, in_menu, updated_at, product_count, subtree_count)
             VALUES ('cat', 'cat', 'Cat', '', '', 0, 0, 1, 1, 2, 2)",
            (),
        )
        .await
        .unwrap();
        for product in ["collar", "basket"] {
            db.execute(
                "INSERT INTO storefront_category_products (category_id, product_id,
                    updated_at, min_amount_cents, max_amount_cents, review_count, rating_x100)
                 SELECT 'cat', id, updated_at, min_amount_cents, max_amount_cents, review_count,
                    rating_x100
                 FROM storefront_products WHERE id = ?1",
                (product,),
            )
            .await
            .unwrap();
        }
        db
    }

    async fn ids(db: &Db, query: ProductQuery) -> (Vec<String>, i64) {
        let page = published_products(db, &query).await.unwrap();
        (page.items.into_iter().map(|p| p.id).collect(), page.total)
    }

    fn owned(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[tokio::test]
    async fn products_by_ids_keeps_the_published_ones_among_them() {
        let db = catalog().await;
        let found = published_products_by_ids(&db, &["basket", "draft", "missing", "collar"])
            .await
            .unwrap();
        let mut ids: Vec<&str> = found.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["basket", "collar"]);
        assert!(
            published_products_by_ids(&db, &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_band_keeps_what_overlaps_it_and_drops_the_unpriced() {
        let db = catalog().await;
        let all = ProductQuery::default().sort(ProductSort::PriceAsc);
        assert_eq!(
            ids(&db, all.clone()).await,
            (owned(&["collar", "basket", "service"]), 3)
        );
        // The collar reaches into both bands; the basket sits in one.
        assert_eq!(
            ids(&db, all.clone().priced_between(Some(0), Some(5000))).await,
            (owned(&["collar", "basket"]), 2)
        );
        assert_eq!(
            ids(&db, all.clone().priced_between(Some(5000), Some(7000))).await,
            (owned(&["collar"]), 1)
        );
        assert_eq!(
            ids(&db, all.clone().priced_between(Some(7000), Some(10000)))
                .await
                .1,
            0
        );
        // Either bound alone.
        assert_eq!(
            ids(&db, all.clone().priced_between(Some(2000), None)).await,
            (owned(&["collar", "basket"]), 2)
        );
        assert_eq!(
            ids(&db, all.priced_between(None, Some(1000))).await,
            (owned(&["collar"]), 1)
        );
    }

    #[tokio::test]
    async fn an_option_keeps_the_products_taking_its_value() {
        let db = catalog().await;
        let all = ProductQuery::default().sort(ProductSort::PriceAsc);
        assert_eq!(
            ids(&db, all.clone().with_option("Couleur", "rose")).await,
            (owned(&["collar"]), 1)
        );
        // Folded both sides: the link's key and the merchant's spelling.
        assert_eq!(
            ids(&db, all.clone().with_option("couleur", "Gris")).await,
            (owned(&["basket"]), 1)
        );
        assert_eq!(
            ids(&db, all.clone().with_option("Couleur", "vert")).await.1,
            0
        );
        // Two pairs must both hold.
        let both = all
            .clone()
            .with_option("Couleur", "Rose")
            .with_option("Couleur", "Bleu");
        assert_eq!(ids(&db, both).await, (owned(&["collar"]), 1));
        let apart = all
            .clone()
            .with_option("Couleur", "Rose")
            .with_option("Couleur", "Gris");
        assert_eq!(ids(&db, apart).await.1, 0);
        // With a band.
        let dear = all
            .with_option("Couleur", "Rose")
            .priced_between(Some(7000), None);
        assert_eq!(ids(&db, dear).await.1, 0);
    }

    #[tokio::test]
    async fn a_filtered_category_counts_what_it_lists() {
        let db = catalog().await;
        let category = ProductQuery::default()
            .in_category("cat")
            .sort(ProductSort::PriceAsc);
        // Unfiltered: the stored count and order.
        assert_eq!(
            ids(&db, category.clone()).await,
            (owned(&["collar", "basket"]), 2)
        );
        assert_eq!(
            ids(&db, category.clone().with_option("Couleur", "Gris")).await,
            (owned(&["basket"]), 1)
        );
        assert_eq!(
            ids(&db, category.priced_between(Some(5000), Some(7000))).await,
            (owned(&["collar"]), 1)
        );
    }

    #[tokio::test]
    async fn the_facet_counts_the_listing_and_sets_its_own_filter_aside() {
        let db = catalog().await;
        async fn values(db: &Db, query: &ProductQuery) -> Vec<(String, String, i64)> {
            published_option_values(db, query, "Couleur")
                .await
                .unwrap()
                .into_iter()
                .map(|v| (v.key, v.label, v.count))
                .collect()
        }
        let all = ProductQuery::default();
        assert_eq!(
            values(&db, &all).await,
            vec![
                ("bleu".into(), "Bleu".into(), 1),
                ("gris".into(), "Gris".into(), 1),
                ("rose".into(), "Rose".into(), 1),
            ]
        );
        // The active colour does not hide the others; the band does narrow.
        assert_eq!(
            values(&db, &all.clone().with_option("Couleur", "rose"))
                .await
                .len(),
            3
        );
        assert_eq!(
            values(&db, &all.clone().priced_between(Some(5000), Some(7000))).await,
            vec![
                ("bleu".into(), "Bleu".into(), 1),
                ("rose".into(), "Rose".into(), 1)
            ]
        );
        // A category, a search, an unknown option.
        let category = all
            .clone()
            .in_category("cat")
            .with_option("Couleur", "gris");
        assert_eq!(values(&db, &category).await.len(), 3);
        assert_eq!(
            values(&db, &all.clone().search("bask")).await,
            vec![("gris".into(), "Gris".into(), 1)]
        );
        assert!(
            published_option_values(&db, &all, "Taille")
                .await
                .unwrap()
                .is_empty()
        );
    }
}

/// The plans and timings of the per-request joins under Turso's planner
/// (#336): a few thousand products in a temporary database opened the way
/// an app opens its own, each statement explained and timed. Run by hand:
/// `cargo test -p timada-storefront probe -- --ignored --nocapture`.
#[cfg(test)]
mod probe {
    use std::time::{Duration, Instant};

    use timada_core::db::{Turso, execute};

    use super::*;
    use crate::migrations::migrate;

    const PRODUCTS: i64 = 3_000;
    const CATEGORIES: i64 = 60;
    const RUNS: u32 = 50;

    /// Three thousand products, four in five published, each filed under two
    /// of sixty categories; the category rows a page reads filled for the
    /// published ones, as the subscription would.
    async fn catalog() -> Db {
        let db = Db::from(Turso::temporary_concurrent().await.unwrap());
        migrate(&db).await.unwrap();
        db.write(&(), |tx, ()| {
            Box::pin(async move {
                for i in 0..PRODUCTS {
                    let id = format!("p{i:05}");
                    let status = if i % 5 == 0 { "draft" } else { "published" };
                    let min = 100 + (i * 37) % 9_000;
                    execute(
                        tx,
                        "INSERT INTO storefront_products (id, slug, name, description,
                            amount_cents, currency, status, updated_at, images_json,
                            options_json, variants_json, attributes_json, min_amount_cents,
                            max_amount_cents, review_count, rating_x100, slug_search,
                            name_search)
                         VALUES (?1, ?1, ?1, '', ?2, 'EUR', ?3, ?4, '[]', '[]', '[]', '[]',
                            ?2, ?5, ?6, ?7, ?1, ?1)",
                        (
                            id.as_str(),
                            min,
                            status,
                            i,
                            min + (i % 7) * 100,
                            i % 11,
                            (i % 5) * 100,
                        ),
                    )
                    .await?;
                    for category in [i % CATEGORIES, (i / 50) % CATEGORIES] {
                        let category = format!("c{category:03}");
                        execute(
                            tx,
                            "INSERT OR IGNORE INTO storefront_product_categories
                                (product_id, category_id) VALUES (?1, ?2)",
                            (id.as_str(), category.as_str()),
                        )
                        .await?;
                        if status == "published" {
                            execute(
                                tx,
                                "INSERT OR IGNORE INTO storefront_category_products
                                    (category_id, product_id, updated_at, min_amount_cents,
                                     max_amount_cents, review_count, rating_x100)
                                 SELECT ?2, id, updated_at, min_amount_cents, max_amount_cents,
                                    review_count, rating_x100
                                 FROM storefront_products WHERE id = ?1",
                                (id.as_str(), category.as_str()),
                            )
                            .await?;
                        }
                    }
                }
                for c in 0..CATEGORIES {
                    let id = format!("c{c:03}");
                    execute(
                        tx,
                        "INSERT INTO storefront_categories (id, slug, name, description, body,
                            parent_id, position, archived, in_menu, updated_at)
                         VALUES (?1, ?1, ?1, '', '', NULL, ?2, ?3, 1, 1)",
                        (id.as_str(), c, i64::from(c % 10 == 9)),
                    )
                    .await?;
                }
                Ok(())
            })
        })
        .await
        .unwrap();
        db
    }

    /// What `EXPLAIN QUERY PLAN` says of `sql`, one line per step.
    async fn plan(db: &Db, sql: &str, params: impl timada_core::db::IntoParams) -> String {
        let rows: Vec<(i64, i64, i64, String)> = db
            .fetch_all(&format!("EXPLAIN QUERY PLAN {sql}"), params)
            .await
            .unwrap();
        rows.into_iter()
            .map(|(_, _, _, detail)| format!("\n    {detail}"))
            .collect()
    }

    /// How long `run` takes on average over [`RUNS`], after one warm-up, and
    /// how many rows it read.
    async fn timed<F, Fut>(mut run: F) -> (Duration, usize)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = usize>,
    {
        run().await;
        let started = Instant::now();
        let mut rows = 0;
        for _ in 0..RUNS {
            rows = run().await;
        }
        (started.elapsed() / RUNS, rows)
    }

    fn report(name: &str, plan: &str, took: Duration, rows: usize) {
        tracing::info!(target: "probe", %name, rows, ?took, plan = %plan);
    }

    const PRODUCT_COLUMNS: &str = "p.id, p.slug, p.name, p.description, p.amount_cents, p.currency,
        p.updated_at, p.images_json, p.options_json, p.variants_json, p.attributes_json,
        p.min_amount_cents, p.max_amount_cents, p.review_count, p.rating_x100, p.stock_json,
        p.in_stock";

    #[tokio::test]
    #[ignore = "measures Turso's plans; run by hand with --ignored --nocapture"]
    async fn category_page() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("probe=info")
            .with_test_writer()
            .try_init();
        let db = catalog().await;
        let category = "c007";
        let (limit, offset) = (24i64, 24i64);

        // As shipped: the join, the page cut on the category rows.
        let sql = CATEGORY_NEWEST;
        let plan_join = plan(&db, sql, (category, limit, offset)).await;
        let (took, rows) = timed(|| async {
            let items: Vec<Product> = db.fetch_all(sql, (category, limit, offset)).await.unwrap();
            items.len()
        })
        .await;
        report("category page: join", &plan_join, took, rows);

        // The ids of the page first, then the products among them, put back
        // in the page's order here.
        let ids_sql = "SELECT product_id FROM storefront_category_products
             WHERE category_id = ?1
             ORDER BY updated_at DESC, product_id DESC LIMIT ?2 OFFSET ?3";
        let by_ids_sql = format!(
            "SELECT {PRODUCT_COLUMNS} FROM storefront_products p
             WHERE p.id IN (SELECT value FROM json_each(?1))"
        );
        let plan_two = format!(
            "{}\n  then{}",
            plan(&db, ids_sql, (category, limit, offset)).await,
            plan(&db, &by_ids_sql, ("[\"p00001\"]",)).await
        );
        let (took, rows) = timed(|| async {
            let ids: Vec<(String,)> = db
                .fetch_all(ids_sql, (category, limit, offset))
                .await
                .unwrap();
            let ids: Vec<&str> = ids.iter().map(|(id,)| id.as_str()).collect();
            let json = serde_json::to_string(&ids).unwrap();
            let mut items: Vec<Product> = db.fetch_all(&by_ids_sql, (json,)).await.unwrap();
            items.sort_by_key(|p| ids.iter().position(|id| *id == p.id));
            items.len()
        })
        .await;
        report("category page: two statements", &plan_two, took, rows);

        // The join written the other way round, should the planner follow
        // the text.
        let sql = format!(
            "SELECT {PRODUCT_COLUMNS}
             FROM storefront_products p
             JOIN storefront_category_products cp ON cp.product_id = p.id
             WHERE cp.category_id = ?1
             ORDER BY cp.updated_at DESC, cp.product_id DESC LIMIT ?2 OFFSET ?3"
        );
        let plan_reversed = plan(&db, &sql, (category, limit, offset)).await;
        let (took, rows) = timed(|| async {
            let items: Vec<Product> = db.fetch_all(&sql, (category, limit, offset)).await.unwrap();
            items.len()
        })
        .await;
        report(
            "category page: join, products first",
            &plan_reversed,
            took,
            rows,
        );

        // A `CROSS JOIN` tells SQLite's planner to keep the order written.
        let sql = format!(
            "SELECT {PRODUCT_COLUMNS}
             FROM storefront_category_products cp
             CROSS JOIN storefront_products p ON p.id = cp.product_id
             WHERE cp.category_id = ?1
             ORDER BY cp.updated_at DESC, cp.product_id DESC LIMIT ?2 OFFSET ?3"
        );
        let plan_cross = plan(&db, &sql, (category, limit, offset)).await;
        let (took, rows) = timed(|| async {
            let items: Vec<Product> = db.fetch_all(&sql, (category, limit, offset)).await.unwrap();
            items.len()
        })
        .await;
        report("category page: cross join", &plan_cross, took, rows);
    }

    #[tokio::test]
    #[ignore = "measures Turso's plans; run by hand with --ignored --nocapture"]
    async fn product_categories() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("probe=info")
            .with_test_writer()
            .try_init();
        let db = catalog().await;
        let product = "p01234";

        // As shipped: the join.
        let sql = "SELECT c.id, c.slug, c.name, c.description, c.parent_id, c.position, c.in_menu
             FROM storefront_categories c
             JOIN storefront_product_categories pc ON pc.category_id = c.id
             WHERE pc.product_id = ?1 AND c.archived = 0
             ORDER BY c.position, c.name, c.id";
        let plan_join = plan(&db, sql, (product,)).await;
        let (took, rows) = timed(|| async {
            let items: Vec<Category> = db.fetch_all(sql, (product,)).await.unwrap();
            items.len()
        })
        .await;
        report("product categories: join", &plan_join, took, rows);

        // The links as a subquery the categories are looked up in.
        let sql = "SELECT c.id, c.slug, c.name, c.description, c.parent_id, c.position, c.in_menu
             FROM storefront_categories c
             WHERE c.archived = 0
               AND c.id IN (SELECT category_id FROM storefront_product_categories
                            WHERE product_id = ?1)
             ORDER BY c.position, c.name, c.id";
        let plan_in = plan(&db, sql, (product,)).await;
        let (took, rows) = timed(|| async {
            let items: Vec<Category> = db.fetch_all(sql, (product,)).await.unwrap();
            items.len()
        })
        .await;
        report("product categories: IN subquery", &plan_in, took, rows);

        // An `EXISTS` over the links, one per category.
        let sql = "SELECT c.id, c.slug, c.name, c.description, c.parent_id, c.position, c.in_menu
             FROM storefront_categories c
             WHERE c.archived = 0
               AND EXISTS (SELECT 1 FROM storefront_product_categories pc
                           WHERE pc.product_id = ?1 AND pc.category_id = c.id)
             ORDER BY c.position, c.name, c.id";
        let plan_exists = plan(&db, sql, (product,)).await;
        let (took, rows) = timed(|| async {
            let items: Vec<Category> = db.fetch_all(sql, (product,)).await.unwrap();
            items.len()
        })
        .await;
        report("product categories: EXISTS", &plan_exists, took, rows);

        // Two statements joined here.
        let links_sql =
            "SELECT category_id FROM storefront_product_categories WHERE product_id = ?1";
        let categories_sql = "SELECT id, slug, name, description, parent_id, position, in_menu
             FROM storefront_categories
             WHERE archived = 0 AND id IN (SELECT value FROM json_each(?1))
             ORDER BY position, name, id";
        let plan_two = format!(
            "{}\n  then{}",
            plan(&db, links_sql, (product,)).await,
            plan(&db, categories_sql, ("[\"c001\"]",)).await
        );
        let (took, rows) = timed(|| async {
            let links: Vec<(String,)> = db.fetch_all(links_sql, (product,)).await.unwrap();
            let ids: Vec<&str> = links.iter().map(|(id,)| id.as_str()).collect();
            let json = serde_json::to_string(&ids).unwrap();
            let items: Vec<Category> = db.fetch_all(categories_sql, (json,)).await.unwrap();
            items.len()
        })
        .await;
        report("product categories: two statements", &plan_two, took, rows);
    }

    /// Not one of the two joins, but the shape the two-statement rewrite
    /// would lean on, and what [`published_products_by_ids`] runs for a
    /// cart: `id IN (SELECT value FROM json_each(?1))`.
    #[tokio::test]
    #[ignore = "measures Turso's plans; run by hand with --ignored --nocapture"]
    async fn products_by_ids() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("probe=info")
            .with_test_writer()
            .try_init();
        let db = catalog().await;
        let ids: Vec<String> = (1..=6).map(|i| format!("p{:05}", i * 401)).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let json = serde_json::to_string(&ids).unwrap();

        // As shipped.
        let sql = format!(
            "SELECT {PRODUCT_COLUMNS} FROM storefront_products p
             WHERE p.id IN (SELECT value FROM json_each(?1)) AND p.status = 'published'"
        );
        let plan_json = plan(&db, &sql, (json.as_str(),)).await;
        let (took, rows) = timed(|| async {
            let items: Vec<Product> = db.fetch_all(&sql, (json.as_str(),)).await.unwrap();
            items.len()
        })
        .await;
        report("products by ids: IN json_each", &plan_json, took, rows);

        // One placeholder per id.
        let sql = format!(
            "SELECT {PRODUCT_COLUMNS} FROM storefront_products p
             WHERE p.id IN ({}) AND p.status = 'published'",
            timada_core::db::placeholders(ids.len())
        );
        let params = || {
            timada_core::db::params_from_iter(
                ids.iter()
                    .map(|id| timada_core::db::Value::Text((*id).to_owned())),
            )
        };
        let plan_list = plan(&db, &sql, params()).await;
        let (took, rows) = timed(|| async {
            let items: Vec<Product> = db.fetch_all(&sql, params()).await.unwrap();
            items.len()
        })
        .await;
        report("products by ids: placeholders", &plan_list, took, rows);

        // The ids read from the JSON first, then one lookup each.
        let one_sql = format!(
            "SELECT {PRODUCT_COLUMNS} FROM storefront_products p
             WHERE p.id = ?1 AND p.status = 'published'"
        );
        let plan_each = plan(&db, &one_sql, ("p00401",)).await;
        let (took, rows) = timed(|| async {
            let mut items: Vec<Product> = Vec::new();
            for id in &ids {
                if let Some(p) = db.fetch_optional(&one_sql, (*id,)).await.unwrap() {
                    items.push(p);
                }
            }
            items.len()
        })
        .await;
        report("products by ids: one lookup each", &plan_each, took, rows);
    }
}
