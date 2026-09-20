//! SQL read model of the category tree, `catalog_category`: one row per
//! category, fed by the `catalog-category-list` subscription. Lists, the tree
//! and the way up from a category (its breadcrumb) all come from here.

use std::collections::{HashMap, HashSet};

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        CategoryArchived, CategoryCreated, CategoryDescribed, CategoryMoved, CategoryPositioned,
        CategoryRenamed,
    },
    command::{Command, MAX_CATEGORY_DEPTH},
    read_model::ProductListRow,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const CATEGORY_LIST_SUBSCRIPTION: &str = "catalog-category-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CategoryRow {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub parent_id: Option<String>,
    pub position: i64,
    pub archived: bool,
}

/// A category with the categories under it, siblings in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryNode {
    pub category: CategoryRow,
    pub children: Vec<CategoryNode>,
}

pub fn category_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CATEGORY_LIST_SUBSCRIPTION)
        .handler(refresh_on_category_created())
        .handler(refresh_on_category_renamed())
        .handler(refresh_on_category_described())
        .handler(refresh_on_category_moved())
        .handler(refresh_on_category_positioned())
        .handler(refresh_on_category_archived())
        .strict()
}

/// Every category, siblings in display order (position, then name).
pub async fn list_categories(
    db: &SqlitePool,
    include_archived: bool,
) -> sqlx::Result<Vec<CategoryRow>> {
    sqlx::query_as(
        "SELECT id, slug, name, description, parent_id, position, archived FROM catalog_category
         WHERE ?1 OR archived = 0
         ORDER BY position, name, id",
    )
    .bind(include_archived)
    .fetch_all(db)
    .await
}

pub async fn category_by_id(db: &SqlitePool, id: &str) -> sqlx::Result<Option<CategoryRow>> {
    sqlx::query_as("SELECT id, slug, name, description, parent_id, position, archived FROM catalog_category WHERE id = ?")
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn category_by_slug(db: &SqlitePool, slug: &str) -> sqlx::Result<Option<CategoryRow>> {
    sqlx::query_as("SELECT id, slug, name, description, parent_id, position, archived FROM catalog_category WHERE slug = ?")
    .bind(slug)
    .fetch_optional(db)
    .await
}

/// The way down to a category: its root first, the category itself last —
/// its breadcrumb. Empty when the category is unknown.
pub async fn category_lineage(db: &SqlitePool, id: &str) -> sqlx::Result<Vec<CategoryRow>> {
    // The depth cap keeps a tree that two operators managed to knot from
    // looping for ever.
    sqlx::query_as(
        "WITH RECURSIVE up(id, depth) AS (
            SELECT ?1, 0
            UNION ALL
            SELECT c.parent_id, up.depth + 1
            FROM up JOIN catalog_category c ON c.id = up.id
            WHERE c.parent_id IS NOT NULL AND up.depth < ?2
         )
         SELECT id, slug, name, description, parent_id, position, archived FROM catalog_category
         JOIN up USING (id)
         ORDER BY up.depth DESC",
    )
    .bind(id)
    .bind(MAX_CATEGORY_DEPTH as i64)
    .fetch_all(db)
    .await
}

/// A category's id and those of everything open under it: what "the products
/// of a category" spans.
pub async fn category_subtree_ids(db: &SqlitePool, id: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "WITH RECURSIVE down(id, depth) AS (
            SELECT ?1, 0
            UNION
            SELECT c.id, down.depth + 1
            FROM down JOIN catalog_category c ON c.parent_id = down.id
            WHERE c.archived = 0 AND down.depth < ?2
         )
         SELECT DISTINCT id FROM down",
    )
    .bind(id)
    .bind(MAX_CATEGORY_DEPTH as i64)
    .fetch_all(db)
    .await
}

/// Whether the storefront shows a category: open, like everything above it.
pub fn is_on_storefront(lineage: &[CategoryRow]) -> bool {
    !lineage.is_empty() && lineage.iter().all(|category| !category.archived)
}

/// How many products on sale are filed *directly* under each category.
pub async fn product_counts_by_category(db: &SqlitePool) -> sqlx::Result<HashMap<String, i64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT category_id, COUNT(*) FROM catalog_product
         WHERE category_id IS NOT NULL AND archived = 0
         GROUP BY category_id",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

fn in_categories(
    select: &'static str,
    category_ids: &[String],
) -> sqlx::QueryBuilder<sqlx::Sqlite> {
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(select);
    query.push(" FROM catalog_product WHERE archived = 0 AND category_id IN (");
    let mut bound = query.separated(", ");
    for id in category_ids {
        bound.push_bind(id);
    }
    query.push(")");
    query
}

/// The products on sale filed under any of `category_ids`, by name.
pub async fn products_in_categories(
    db: &SqlitePool,
    category_ids: &[String],
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<ProductListRow>> {
    if category_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = in_categories(
        "SELECT id, sku, name, brand_slug, category_path, archived, category_id",
        category_ids,
    );
    query.push(" ORDER BY name, id LIMIT ");
    query.push_bind(limit);
    query.push(" OFFSET ");
    query.push_bind(offset);
    query.build_query_as().fetch_all(db).await
}

pub async fn count_products_in_categories(
    db: &SqlitePool,
    category_ids: &[String],
) -> sqlx::Result<i64> {
    if category_ids.is_empty() {
        return Ok(0);
    }
    in_categories("SELECT COUNT(*)", category_ids)
        .build_query_scalar()
        .fetch_one(db)
        .await
}

/// Nests the rows of [`list_categories`] (their order is kept among
/// siblings). Without `include_archived`, an archived category goes with
/// everything under it. A category whose parent is missing from `rows` — or
/// that a knot in the tree made unreachable — is left out.
pub fn category_tree(rows: Vec<CategoryRow>, include_archived: bool) -> Vec<CategoryNode> {
    let mut by_parent: HashMap<Option<String>, Vec<CategoryRow>> = HashMap::new();
    for row in rows {
        if include_archived || !row.archived {
            by_parent
                .entry(row.parent_id.clone())
                .or_default()
                .push(row);
        }
    }
    fn grow(
        parent: Option<String>,
        by_parent: &mut HashMap<Option<String>, Vec<CategoryRow>>,
        seen: &mut HashSet<String>,
        depth: usize,
    ) -> Vec<CategoryNode> {
        if depth > MAX_CATEGORY_DEPTH {
            return Vec::new();
        }
        let rows = by_parent.remove(&parent).unwrap_or_default();
        let mut nodes = Vec::with_capacity(rows.len());
        for category in rows {
            if !seen.insert(category.id.clone()) {
                continue;
            }
            let children = grow(Some(category.id.clone()), by_parent, seen, depth + 1);
            nodes.push(CategoryNode { category, children });
        }
        nodes
    }
    grow(None, &mut by_parent, &mut HashSet::new(), 1)
}

impl CategoryNode {
    /// The node and everything under it, depth first, each with its depth
    /// (this node being `depth`): what an indented list or a `<select>` shows.
    pub fn flatten(&self, depth: usize) -> Vec<(usize, &CategoryRow)> {
        let mut flat = vec![(depth, &self.category)];
        for child in &self.children {
            flat.extend(child.flatten(depth + 1));
        }
        flat
    }
}

/// Rewrites a category's row with what the category is *now* — absolute
/// values, so a redelivery changes nothing.
async fn refresh<E: Executor>(ctx: &Context<'_, E>, id: &str) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(category) = Command(ctx.executor).load_category(id).await? else {
        anyhow::bail!("category {id} has events but cannot be loaded");
    };
    sqlx::query(
        "INSERT INTO catalog_category (id, slug, name, description, parent_id, position, archived)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE
         SET name = excluded.name, description = excluded.description,
             parent_id = excluded.parent_id, position = excluded.position,
             archived = excluded.archived",
    )
    .bind(&category.id)
    .bind(&category.slug)
    .bind(&category.name)
    .bind(&category.description)
    .bind(&category.parent_id)
    .bind(category.position)
    .bind(category.archived)
    .execute(&db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_category_created<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CategoryCreated>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_category_renamed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CategoryRenamed>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_category_described<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CategoryDescribed>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_category_moved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CategoryMoved>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_category_positioned<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CategoryPositioned>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_category_archived<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CategoryArchived>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}
