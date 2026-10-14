//! What a storefront page reads: the published products of the local copy.

use sqlx::{FromRow, SqlitePool};
use timada_catalog::{TreeNode, TreeOf, format_money, tree_of};

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
}

impl Product {
    /// The price as a person reads it (`12.50 EUR`), once it has one.
    pub fn price(&self) -> Option<String> {
        Some(format_money(
            self.amount_cents? as u64,
            self.currency.as_deref()?,
        ))
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
/// [`MAX_PER_PAGE`], an optional search over name and slug, the categories a
/// product must be in (any of them), one product to leave out and the order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductQuery {
    pub page: u32,
    pub per_page: u32,
    pub q: Option<String>,
    /// Empty means every category.
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

    /// Only products in one of these categories. Pass a
    /// [`CategoryNode::subtree_ids`] to include its sub-categories.
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

/// What a page of products is filtered by, shared by the count and the list
/// so the two never disagree. `?1` is the search pattern, `?2` the JSON array
/// of category ids (`[]` for every category), `?3` a product to leave out.
macro_rules! products_where {
    () => {
        "WHERE status = 'published'
           AND (?1 IS NULL OR slug LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\')
           AND (?2 = '[]' OR id IN (SELECT product_id FROM storefront_product_categories
                WHERE category_id IN (SELECT value FROM json_each(?2))))
           AND (?3 IS NULL OR id <> ?3) "
    };
}

/// One page of the list, ordered by `$order`. The SQL has to be a literal
/// (sqlx refuses a built string), so each order is its own statement over the
/// same filter.
macro_rules! products_page {
    ($order:literal) => {
        concat!(
            "SELECT id, slug, name, description, amount_cents, currency, updated_at
             FROM storefront_products ",
            products_where!(),
            $order,
            " LIMIT ?4 OFFSET ?5"
        )
    };
}

const PAGE_NEWEST: &str = products_page!("ORDER BY updated_at DESC, id DESC");
/// A product with no price sorts last either way.
const PAGE_PRICE_ASC: &str =
    products_page!("ORDER BY amount_cents IS NULL, amount_cents ASC, id DESC");
const PAGE_PRICE_DESC: &str =
    products_page!("ORDER BY amount_cents IS NULL, amount_cents DESC, id DESC");
const COUNT: &str = concat!(
    "SELECT count(*) FROM storefront_products ",
    products_where!()
);

/// The published products a query matches, in the order it asks for.
pub async fn published_products(
    pool: &SqlitePool,
    query: &ProductQuery,
) -> sqlx::Result<ProductPage> {
    let per_page = i64::from(query.per_page.clamp(1, MAX_PER_PAGE));
    let page = query.page.max(1);
    let pattern = query.q.as_deref().map(contains);
    // A JSON array, read back by `json_each`: one bind for any number of ids.
    let categories = serde_json::to_string(&query.categories).unwrap_or_else(|_| "[]".to_owned());
    let total: i64 = sqlx::query_scalar(COUNT)
        .bind(&pattern)
        .bind(&categories)
        .bind(query.exclude.as_deref())
        .fetch_one(pool)
        .await?;
    let statement = match query.sort {
        ProductSort::Newest => PAGE_NEWEST,
        ProductSort::PriceAsc => PAGE_PRICE_ASC,
        ProductSort::PriceDesc => PAGE_PRICE_DESC,
    };
    let items = sqlx::query_as(statement)
        .bind(&pattern)
        .bind(&categories)
        .bind(query.exclude.as_deref())
        .bind(per_page)
        .bind(i64::from(page - 1) * per_page)
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
        "SELECT id, slug, name, description, amount_cents, currency, updated_at
         FROM storefront_products WHERE slug = ?1 AND status = 'published'",
    )
    .bind(slug)
    .fetch_optional(pool)
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
    /// This category's id and every one below it: what a collection page
    /// lists.
    pub fn subtree_ids(&self) -> Vec<String> {
        let mut ids = vec![self.category.id.clone()];
        for child in &self.children {
            ids.extend(child.subtree_ids());
        }
        ids
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

/// The live categories above `category`, top-level first: a breadcrumb.
/// Stops where a parent is missing or archived.
pub async fn category_ancestors(
    pool: &SqlitePool,
    category: &Category,
) -> sqlx::Result<Vec<Category>> {
    let mut chain: Vec<Category> = Vec::new();
    let mut next = category.parent_id.clone();
    while let Some(parent_id) = next {
        if chain.iter().any(|seen| seen.id == parent_id) || parent_id == category.id {
            break;
        }
        let parent: Option<Category> = sqlx::query_as(
            "SELECT id, slug, name, description, parent_id, position
             FROM storefront_categories WHERE id = ?1 AND archived = 0",
        )
        .bind(&parent_id)
        .fetch_optional(pool)
        .await?;
        let Some(parent) = parent else { break };
        next = parent.parent_id.clone();
        chain.push(parent);
        if chain.len() > 16 {
            break;
        }
    }
    chain.reverse();
    Ok(chain)
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
