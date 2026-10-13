//! Queries over `catalog_categories`, for the category pages of a tenant.
//!
//! The tree is assembled in memory: a tenant has tens of categories, not
//! thousands, and one flat query beats a recursive one.

use sqlx::{FromRow, SqlitePool};

/// One row of `catalog_categories`, with how many products are in it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct CategoryRow {
    pub tenant_id: String,
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub parent_id: Option<String>,
    pub position: i64,
    pub archived: bool,
    pub created_by: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
    /// Products of this very category that are not archived.
    pub product_count: i64,
}

/// What a tree is built of: anything that knows its id, its parent and where
/// it sorts.
pub trait TreeNode {
    fn node_id(&self) -> &str;
    fn node_parent_id(&self) -> Option<&str>;
}

impl TreeNode for CategoryRow {
    fn node_id(&self) -> &str {
        &self.id
    }

    fn node_parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }
}

/// A node of the tree and what hangs under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeOf<T> {
    pub item: T,
    pub children: Vec<TreeOf<T>>,
}

impl<T> TreeOf<T> {
    /// The node and its descendants, breadth-first.
    pub fn flatten(&self) -> Vec<&TreeOf<T>> {
        let mut out = vec![self];
        let mut i = 0;
        while i < out.len() {
            let node: &TreeOf<T> = out[i];
            out.extend(node.children.iter());
            i += 1;
        }
        out
    }
}

/// A tree of `items`, keeping their order within each level. An item whose
/// parent is not in `items` (archived, or filtered out) becomes a root, so
/// nothing is ever dropped.
pub fn tree_of<T: TreeNode>(items: Vec<T>) -> Vec<TreeOf<T>> {
    let known: std::collections::HashSet<String> =
        items.iter().map(|i| i.node_id().to_owned()).collect();
    let mut nodes: Vec<Option<TreeOf<T>>> = Vec::with_capacity(items.len());
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut parents: Vec<Option<String>> = Vec::with_capacity(items.len());
    for (position, item) in items.into_iter().enumerate() {
        index.insert(item.node_id().to_owned(), position);
        let parent = item
            .node_parent_id()
            .filter(|p| known.contains(*p))
            .map(str::to_owned);
        parents.push(parent);
        nodes.push(Some(TreeOf {
            item,
            children: Vec::new(),
        }));
    }
    // Deepest first, so a node is finished before it is moved into its
    // parent. The sort is stable, so siblings keep the order they came in.
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|position| std::cmp::Reverse(depth_of(*position, &parents, &index)));
    let mut roots: Vec<(usize, TreeOf<T>)> = Vec::new();
    for position in order {
        let Some(node) = nodes[position].take() else {
            continue;
        };
        match parents[position]
            .as_deref()
            .and_then(|parent| index.get(parent))
        {
            Some(&parent) => {
                if let Some(parent) = nodes[parent].as_mut() {
                    parent.children.push(node);
                } else {
                    roots.push((position, node));
                }
            }
            None => roots.push((position, node)),
        }
    }
    roots.sort_by_key(|(position, _)| *position);
    roots.into_iter().map(|(_, node)| node).collect()
}

/// How far a node sits from its root, stopping at a loop.
fn depth_of(
    position: usize,
    parents: &[Option<String>],
    index: &std::collections::HashMap<String, usize>,
) -> usize {
    let mut depth = 0;
    let mut at = position;
    while let Some(parent) = parents[at].as_deref().and_then(|p| index.get(p)) {
        depth += 1;
        at = *parent;
        if depth > parents.len() {
            break;
        }
    }
    depth
}

/// Every `(category id, product id)` of a tenant whose product is not
/// archived: what [`subtree_counts`] needs to count a category's page.
pub async fn memberships_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
) -> sqlx::Result<Vec<(String, String)>> {
    sqlx::query_as(
        "SELECT pc.category_id, pc.product_id
         FROM catalog_product_categories pc
         JOIN catalog_products p ON p.tenant_id = pc.tenant_id AND p.id = pc.product_id
         WHERE pc.tenant_id = ?1 AND p.status <> 'archived'",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
}

/// How many distinct products each category's page lists: those filed under
/// it and under everything below it, each counted once however many of the
/// categories it belongs to.
///
/// Keyed by category id; a category not in `nodes` is absent.
pub fn subtree_counts<T: TreeNode>(
    nodes: &[TreeOf<T>],
    memberships: &[(String, String)],
) -> std::collections::HashMap<String, i64> {
    let mut by_category: std::collections::HashMap<&str, Vec<&str>> =
        std::collections::HashMap::new();
    for (category_id, product_id) in memberships {
        by_category
            .entry(category_id.as_str())
            .or_default()
            .push(product_id.as_str());
    }
    let mut counts = std::collections::HashMap::new();
    for node in nodes {
        count_subtree(node, &by_category, &mut counts);
    }
    counts
}

/// Counts one node, bottom-up, and hands its products back to its parent so
/// a product in both is counted once.
fn count_subtree<T: TreeNode>(
    node: &TreeOf<T>,
    by_category: &std::collections::HashMap<&str, Vec<&str>>,
    counts: &mut std::collections::HashMap<String, i64>,
) -> std::collections::HashSet<String> {
    let mut products: std::collections::HashSet<String> = by_category
        .get(node.item.node_id())
        .map(|ids| ids.iter().map(|id| (*id).to_owned()).collect())
        .unwrap_or_default();
    for child in &node.children {
        products.extend(count_subtree(child, by_category, counts));
    }
    counts.insert(node.item.node_id().to_owned(), products.len() as i64);
    products
}

/// The live categories of a tenant, parents before children, each level in
/// its own order. `product_count` counts the products filed under that very
/// category that are not archived; what its page lists is
/// [`subtree_counts`].
pub async fn categories_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
) -> sqlx::Result<Vec<CategoryRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, slug, name, description, parent_id, position, archived,
            created_by, created_at, updated_at,
            (SELECT count(*) FROM catalog_product_categories pc
                JOIN catalog_products p ON p.tenant_id = pc.tenant_id AND p.id = pc.product_id
                WHERE pc.tenant_id = c.tenant_id AND pc.category_id = c.id
                  AND p.status <> 'archived') AS product_count
         FROM catalog_categories c
         WHERE tenant_id = ?1 AND archived = 0
         ORDER BY position, name, id",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
}

/// One category of a tenant, by id, archived or not.
pub async fn category_by_id(
    pool: &SqlitePool,
    tenant_id: &str,
    id: &str,
) -> sqlx::Result<Option<CategoryRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, slug, name, description, parent_id, position, archived,
            created_by, created_at, updated_at,
            (SELECT count(*) FROM catalog_product_categories pc
                JOIN catalog_products p ON p.tenant_id = pc.tenant_id AND p.id = pc.product_id
                WHERE pc.tenant_id = c.tenant_id AND pc.category_id = c.id
                  AND p.status <> 'archived') AS product_count
         FROM catalog_categories c
         WHERE tenant_id = ?1 AND id = ?2",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// How many live categories sit directly under this one.
pub async fn live_child_count(pool: &SqlitePool, tenant_id: &str, id: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM catalog_categories
         WHERE tenant_id = ?1 AND parent_id = ?2 AND archived = 0",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_one(pool)
    .await
}

/// The categories a product is in. Live ones come first, in tree order;
/// archived memberships come last, so a page can show them as such.
pub async fn categories_of_product(
    pool: &SqlitePool,
    tenant_id: &str,
    product_id: &str,
) -> sqlx::Result<Vec<CategoryRow>> {
    sqlx::query_as(
        "SELECT c.tenant_id, c.id, c.slug, c.name, c.description, c.parent_id, c.position,
            c.archived, c.created_by, c.created_at, c.updated_at,
            (SELECT count(*) FROM catalog_product_categories pc2
                JOIN catalog_products p ON p.tenant_id = pc2.tenant_id AND p.id = pc2.product_id
                WHERE pc2.tenant_id = c.tenant_id AND pc2.category_id = c.id
                  AND p.status <> 'archived') AS product_count
         FROM catalog_categories c
         JOIN catalog_product_categories pc
            ON pc.tenant_id = c.tenant_id AND pc.category_id = c.id
         WHERE c.tenant_id = ?1 AND pc.product_id = ?2
         ORDER BY c.archived, c.position, c.name, c.id",
    )
    .bind(tenant_id)
    .bind(product_id)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Node(&'static str, Option<&'static str>);

    impl TreeNode for Node {
        fn node_id(&self) -> &str {
            self.0
        }

        fn node_parent_id(&self) -> Option<&str> {
            self.1
        }
    }

    #[test]
    fn builds_a_tree_in_order() {
        let tree = tree_of(vec![
            Node("a", None),
            Node("a1", Some("a")),
            Node("a2", Some("a")),
            Node("a1x", Some("a1")),
            Node("b", None),
            // Its parent is not in the list: it stands on its own.
            Node("orphan", Some("gone")),
        ]);
        assert_eq!(
            tree.iter().map(|n| n.item.0).collect::<Vec<_>>(),
            ["a", "b", "orphan"]
        );
        assert_eq!(
            tree[0]
                .children
                .iter()
                .map(|n| n.item.0)
                .collect::<Vec<_>>(),
            ["a1", "a2"]
        );
        assert_eq!(tree[0].children[0].children[0].item.0, "a1x");
        assert_eq!(
            tree[0]
                .flatten()
                .iter()
                .map(|n| n.item.0)
                .collect::<Vec<_>>(),
            ["a", "a1", "a2", "a1x"]
        );
    }

    #[test]
    fn counts_a_subtree_once_per_product() {
        let tree = tree_of(vec![
            Node("bedding", None),
            Node("mats", Some("bedding")),
            Node("toys", None),
        ]);
        let memberships = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(c, p)| ((*c).to_owned(), (*p).to_owned()))
                .collect()
        };

        // A product in the child counts for the parent's page too.
        let counts = subtree_counts(&tree, &memberships(&[("mats", "mat")]));
        assert_eq!(counts["bedding"], 1);
        assert_eq!(counts["mats"], 1);
        assert_eq!(counts["toys"], 0);

        // One in both is still one product on the parent's page.
        let counts = subtree_counts(&tree, &memberships(&[("bedding", "mat"), ("mats", "mat")]));
        assert_eq!(counts["bedding"], 1, "not two");
        assert_eq!(counts["mats"], 1);

        // Different products add up.
        let counts = subtree_counts(
            &tree,
            &memberships(&[("bedding", "basket"), ("mats", "mat"), ("toys", "ball")]),
        );
        assert_eq!(counts["bedding"], 2);
        assert_eq!(counts["toys"], 1);
    }

    #[test]
    fn a_loop_does_not_hang() {
        let tree = tree_of(vec![Node("x", Some("y")), Node("y", Some("x"))]);
        assert_eq!(tree.len() + tree[0].children.len(), 2);
    }
}
