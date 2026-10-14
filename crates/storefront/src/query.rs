//! What a storefront page reads: the published products of the local copy.

use sqlx::{FromRow, SqlitePool};
use timada_catalog::{TreeNode, TreeOf, format_money, tree_of};

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
    /// The price as a person reads it (`22.90 EUR`).
    pub fn price(&self) -> String {
        format_money(self.amount_cents.max(0) as u64, &self.currency)
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

    /// `12.50 EUR`, or `12.00–18.00 EUR` when it varies.
    pub fn label(&self) -> String {
        let max = format_money(self.max_cents.max(0) as u64, &self.currency);
        if !self.varies() {
            return max;
        }
        let min = format_money(self.min_cents.max(0) as u64, &self.currency);
        format!("{}–{max}", min.trim_end_matches(&self.currency).trim_end())
    }
}

/// A product as the storefront shows it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq, Hash)]
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
    #[sqlx(json, rename = "images_json")]
    pub images: Vec<Image>,
    /// The ways it varies; empty when it does not.
    #[sqlx(json, rename = "options_json")]
    pub options: Vec<ProductOption>,
    /// The combinations it sells, each with its own price.
    #[sqlx(json, rename = "variants_json")]
    pub variants: Vec<Variant>,
    /// The facts in its table.
    #[sqlx(json, rename = "attributes_json")]
    pub attributes: Vec<Attribute>,
    pub min_amount_cents: Option<i64>,
    pub max_amount_cents: Option<i64>,
}

impl Product {
    /// The price as a listing shows it: `12.50 EUR`, or `12.00–18.00 EUR`
    /// when the variants differ. Empty when it has no price at all.
    pub fn price(&self) -> Option<String> {
        Some(self.price_range()?.label())
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
}

impl ProductSort {
    /// The value a sort `<select>` and a query string use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::PriceAsc => "price_asc",
            Self::PriceDesc => "price_desc",
        }
    }

    /// The sort a query string asks for, or [`Newest`](Self::Newest) for
    /// anything else.
    pub fn parse(value: &str) -> Self {
        match value {
            "price_asc" => Self::PriceAsc,
            "price_desc" => Self::PriceDesc,
            _ => Self::Newest,
        }
    }
}

/// A page of the catalog: 1-based `page`, `per_page` capped at
/// [`MAX_PER_PAGE`], an optional search over name and slug, a category whose
/// page it is, the categories a product must be in (any of them), one product
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
    /// A product to leave out, by id: what "related products" needs.
    pub exclude: Option<String>,
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
            exclude: None,
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
        self.exclude = Some(id.into());
        self
    }

    pub fn sort(mut self, sort: ProductSort) -> Self {
        self.sort = sort;
        self
    }
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

/// A `LIKE` pattern matching `q` anywhere, with its wildcards escaped.
fn contains(q: &str) -> String {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// The full-text phrase matching `q` anywhere in a word, when `q` is long
/// enough for the index (it is cut in runs of three characters); `None`
/// otherwise, and the search falls back to [`contains`].
fn phrase(q: &str) -> Option<String> {
    (q.chars().count() >= 3).then(|| format!("\"{}\"", q.replace('"', "\"\"")))
}

/// Where a page of products comes from, `?1` being what was searched for.
/// With a phrase the full-text index leads, so a search costs what it
/// matches; `CROSS JOIN` keeps the planner from walking every product in
/// sort order instead. Without one, `?1` is a `LIKE` pattern or `NULL`.
macro_rules! products_from {
    (phrase) => {
        "FROM storefront_products_fts f
         CROSS JOIN storefront_products p ON p.rowid = f.rowid
         WHERE storefront_products_fts MATCH ?1 "
    };
    (pattern) => {
        "FROM storefront_products p
         WHERE (?1 IS NULL OR p.slug LIKE ?1 ESCAPE '\\' OR p.name LIKE ?1 ESCAPE '\\') "
    };
}

/// What a page of products is filtered by besides the search, shared by the
/// count and the list so the two never disagree. `?2` is the JSON array of
/// category ids (`[]` for every category), `?3` a product to leave out, `?4`
/// a category whose page it is (read from what the subscription stored for
/// it, so a sub-category costs nothing more).
macro_rules! products_where {
    () => {
        "AND p.status = 'published'
           AND (?2 = '[]' OR p.id IN (SELECT product_id FROM storefront_product_categories
                WHERE category_id IN (SELECT value FROM json_each(?2))))
           AND (?3 IS NULL OR p.id <> ?3)
           AND (?4 IS NULL OR p.id IN (SELECT product_id FROM storefront_category_products
                WHERE category_id = ?4)) "
    };
}

/// One page of the list, ordered by `$order`. The SQL has to be a literal
/// (sqlx refuses a built string), so each order is its own statement over the
/// same filter.
macro_rules! products_page {
    ($from:ident, $order:literal) => {
        concat!(
            "SELECT p.id, p.slug, p.name, p.description, p.amount_cents, p.currency,
                p.updated_at, p.images_json, p.options_json, p.variants_json,
                p.attributes_json, p.min_amount_cents, p.max_amount_cents ",
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
        ]
    };
}

const PAGES: [&str; 3] = products_pages!(pattern);
const COUNT: &str = products_count!(pattern);
const SEARCH_PAGES: [&str; 3] = products_pages!(phrase);
const SEARCH_COUNT: &str = products_count!(phrase);

impl ProductSort {
    /// Its statement among the three of [`products_pages`].
    fn index(self) -> usize {
        match self {
            ProductSort::Newest => 0,
            ProductSort::PriceAsc => 1,
            ProductSort::PriceDesc => 2,
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
                p.attributes_json, p.min_amount_cents, p.max_amount_cents
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

/// The published products a query matches, in the order it asks for.
pub async fn published_products(
    pool: &SqlitePool,
    query: &ProductQuery,
) -> sqlx::Result<ProductPage> {
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
    let offset = i64::from(page - 1) * per_page;
    // A category's page as it is, with nothing else to match: the count and
    // the order the subscription stored.
    if let Some(category) = query.category.as_deref()
        && search.is_none()
        && query.categories.is_empty()
        && query.exclude.is_none()
    {
        let total: i64 =
            sqlx::query_scalar("SELECT subtree_count FROM storefront_categories WHERE id = ?1")
                .bind(category)
                .fetch_optional(pool)
                .await?
                .unwrap_or(0);
        let statement = match query.sort {
            ProductSort::Newest => CATEGORY_NEWEST,
            ProductSort::PriceAsc => CATEGORY_PRICE_ASC,
            ProductSort::PriceDesc => CATEGORY_PRICE_DESC,
        };
        let items = sqlx::query_as(statement)
            .bind(category)
            .bind(per_page)
            .bind(offset)
            .fetch_all(pool)
            .await?;
        return Ok(ProductPage {
            items,
            page,
            per_page: per_page as u32,
            total,
        });
    }
    let total: i64 = sqlx::query_scalar(count)
        .bind(&search)
        .bind(&categories)
        .bind(query.exclude.as_deref())
        .bind(query.category.as_deref())
        .fetch_one(pool)
        .await?;
    let items = sqlx::query_as(pages[query.sort.index()])
        .bind(&search)
        .bind(&categories)
        .bind(query.exclude.as_deref())
        .bind(query.category.as_deref())
        .bind(per_page)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(ProductPage {
        items,
        page,
        per_page: per_page as u32,
        total,
    })
}

/// One published product, by slug.
pub async fn published_product_by_slug(
    pool: &SqlitePool,
    slug: &str,
) -> sqlx::Result<Option<Product>> {
    sqlx::query_as(
        "SELECT id, slug, name, description, amount_cents, currency, updated_at,
            images_json, options_json, variants_json, attributes_json,
            min_amount_cents, max_amount_cents
         FROM storefront_products WHERE slug = ?1 AND status = 'published'",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await
}

/// The published products among `ids`, in no particular order: what a cart
/// page shows its lines with (a product missing here is off sale).
pub async fn published_products_by_ids(
    pool: &SqlitePool,
    ids: &[&str],
) -> sqlx::Result<Vec<Product>> {
    let ids = serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_owned());
    sqlx::query_as(
        "SELECT id, slug, name, description, amount_cents, currency, updated_at,
            images_json, options_json, variants_json, attributes_json,
            min_amount_cents, max_amount_cents
         FROM storefront_products
         WHERE id IN (SELECT value FROM json_each(?1)) AND status = 'published'",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
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

/// A category row with the count its page lists, as the subscription stored
/// it.
#[derive(FromRow)]
struct CountedCategory {
    #[sqlx(flatten)]
    category: Category,
    subtree_count: i64,
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
pub async fn published_categories(pool: &SqlitePool) -> sqlx::Result<Vec<CategoryNode>> {
    let categories: Vec<CountedCategory> = sqlx::query_as(
        "SELECT id, slug, name, description, parent_id, position, subtree_count
         FROM storefront_categories WHERE archived = 0
         ORDER BY position, name, id",
    )
    .fetch_all(pool)
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
pub async fn published_category_by_slug(
    pool: &SqlitePool,
    slug: &str,
) -> sqlx::Result<Option<Category>> {
    sqlx::query_as(
        "SELECT id, slug, name, description, parent_id, position
         FROM storefront_categories WHERE slug = ?1 AND archived = 0",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await
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
    pool: &SqlitePool,
    product_id: &str,
) -> sqlx::Result<Vec<Category>> {
    sqlx::query_as(
        "SELECT c.id, c.slug, c.name, c.description, c.parent_id, c.position
         FROM storefront_categories c
         JOIN storefront_product_categories pc ON pc.category_id = c.id
         WHERE pc.product_id = ?1 AND c.archived = 0
         ORDER BY c.position, c.name, c.id",
    )
    .bind(product_id)
    .fetch_all(pool)
    .await
}

/// The published products sharing a category with `product`, newest first,
/// without `product` itself.
pub async fn published_related_products(
    pool: &SqlitePool,
    product: &Product,
    limit: u32,
) -> sqlx::Result<Vec<Product>> {
    let categories: Vec<(String,)> = sqlx::query_as(
        "SELECT category_id FROM storefront_product_categories WHERE product_id = ?1",
    )
    .bind(&product.id)
    .fetch_all(pool)
    .await?;
    if categories.is_empty() {
        return Ok(Vec::new());
    }
    let query = ProductQuery::default()
        .per_page(limit)
        .in_categories(categories.into_iter().map(|(id,)| id))
        .exclude(product.id.clone());
    Ok(published_products(pool, &query).await?.items)
}
