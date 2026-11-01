//! Queries over `catalog_categories`, for the category pages of a tenant.
//!
//! The tree is assembled in memory: a tenant has tens of categories, not
//! thousands, and one flat query beats a recursive one. The counts a page
//! shows are not: they are worked out when a product or a category changes
//! ([`category_counts`], called by the read-model subscription) and stored on
//! the row, so reading a category costs one plain `SELECT`.

use anyhow::Result;
use timada_core::db::{Connection, Db, FromRow, execute, fetch_all};

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
    /// Shown in the shop's menu; a hidden one takes its subtree with it.
    pub in_menu: bool,
    pub created_by: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
    /// Products of this very category that are not archived.
    pub product_count: i64,
    /// Distinct products that are not archived in it and in every live
    /// category below it: what its storefront page lists. Equal to
    /// `product_count` for a category with nothing below it.
    pub subtree_count: i64,
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

/// How many distinct products each category's page lists: those filed under
/// it and under everything below it, each counted once however many of the
/// categories it belongs to.
///
/// Keyed by category id; a category not in `nodes` is absent.
pub fn subtree_counts<T: TreeNode>(
    nodes: &[TreeOf<T>],
    memberships: &[(String, String)],
) -> std::collections::HashMap<String, i64> {
    subtree_products(nodes, memberships)
        .into_iter()
        .map(|(id, products)| (id, products.len() as i64))
        .collect()
}

/// The distinct products each category's page lists: those filed under it
/// and under everything below it.
///
/// Keyed by category id; a category not in `nodes` is absent.
pub fn subtree_products<T: TreeNode>(
    nodes: &[TreeOf<T>],
    memberships: &[(String, String)],
) -> std::collections::HashMap<String, std::collections::BTreeSet<String>> {
    let mut by_category: std::collections::HashMap<&str, Vec<&str>> =
        std::collections::HashMap::new();
    for (category_id, product_id) in memberships {
        by_category
            .entry(category_id.as_str())
            .or_default()
            .push(product_id.as_str());
    }
    let mut out = std::collections::HashMap::new();
    for node in nodes {
        collect_subtree(node, &by_category, &mut out);
    }
    out
}

/// Collects one node, bottom-up, and hands its products back to its parent
/// so a product in both is listed once.
fn collect_subtree<T: TreeNode>(
    node: &TreeOf<T>,
    by_category: &std::collections::HashMap<&str, Vec<&str>>,
    out: &mut std::collections::HashMap<String, std::collections::BTreeSet<String>>,
) -> std::collections::BTreeSet<String> {
    let mut products: std::collections::BTreeSet<String> = by_category
        .get(node.item.node_id())
        .map(|ids| ids.iter().map(|id| (*id).to_owned()).collect())
        .unwrap_or_default();
    for child in &node.children {
        products.extend(collect_subtree(child, by_category, out));
    }
    out.insert(node.item.node_id().to_owned(), products.clone());
    products
}

/// A category as far as counting goes: where it hangs, and whether it is
/// still in the tree.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct CategoryLink {
    pub id: String,
    pub parent_id: Option<String>,
    pub archived: bool,
}

impl TreeNode for CategoryLink {
    fn node_id(&self) -> &str {
        &self.id
    }

    fn node_parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }
}

/// What is stored about a category's products.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategoryCounts {
    /// Products filed under that very category.
    pub product_count: i64,
    /// Distinct products in it and in every live category below it.
    pub subtree_count: i64,
    /// Those products, by id: what its page lists.
    pub subtree_products: std::collections::BTreeSet<String>,
}

/// The counts of every category in `links`, from the `(category id, product
/// id)` pairs of the products that count. An archived category is out of the
/// tree: its page lists nothing below it, so both its numbers are what is
/// filed under it.
pub fn category_counts(
    links: Vec<CategoryLink>,
    memberships: &[(String, String)],
) -> std::collections::HashMap<String, CategoryCounts> {
    let mut direct: std::collections::HashMap<&str, std::collections::BTreeSet<String>> =
        std::collections::HashMap::new();
    for (category_id, product_id) in memberships {
        direct
            .entry(category_id.as_str())
            .or_default()
            .insert(product_id.clone());
    }
    let mut out: std::collections::HashMap<String, CategoryCounts> = links
        .iter()
        .map(|link| {
            let products = direct.get(link.id.as_str()).cloned().unwrap_or_default();
            (
                link.id.clone(),
                CategoryCounts {
                    product_count: products.len() as i64,
                    subtree_count: products.len() as i64,
                    subtree_products: products,
                },
            )
        })
        .collect();
    let live: Vec<CategoryLink> = links.into_iter().filter(|link| !link.archived).collect();
    for (id, products) in subtree_products(&tree_of(live), memberships) {
        if let Some(counts) = out.get_mut(&id) {
            counts.subtree_count = products.len() as i64;
            counts.subtree_products = products;
        }
    }
    out
}

/// Works out and stores the counts of every category of a tenant, inside the
/// transaction that changed what they count. Only a category whose counts
/// moved is written: a product's write runs this, and a statement costs
/// about as much whether it changes a row or not.
pub async fn refresh_category_counts(conn: &Connection, tenant_id: &str) -> Result<()> {
    let rows: Vec<(String, Option<String>, bool, i64, i64)> = fetch_all(
        conn,
        "SELECT id, parent_id, archived, product_count, subtree_count
         FROM catalog_categories WHERE tenant_id = ?1",
        (tenant_id,),
    )
    .await?;
    let stored: std::collections::HashMap<&str, (i64, i64)> = rows
        .iter()
        .map(|(id, _, _, product_count, subtree_count)| {
            (id.as_str(), (*product_count, *subtree_count))
        })
        .collect();
    let links: Vec<CategoryLink> = rows
        .iter()
        .map(|(id, parent_id, archived, _, _)| CategoryLink {
            id: id.clone(),
            parent_id: parent_id.clone(),
            archived: *archived,
        })
        .collect();
    // Two scans joined here rather than one SQL join: Turso answers that
    // join by walking every product of the tenant once per link, which
    // made a recount of a 1 300-product shop take seconds.
    let archived: std::collections::HashSet<String> = fetch_all::<(String,)>(
        conn,
        "SELECT id FROM catalog_products WHERE tenant_id = ?1 AND status = 'archived'",
        (tenant_id,),
    )
    .await?
    .into_iter()
    .map(|(id,)| id)
    .collect();
    let memberships: Vec<(String, String)> = fetch_all::<(String, String)>(
        conn,
        "SELECT category_id, product_id FROM catalog_product_categories WHERE tenant_id = ?1",
        (tenant_id,),
    )
    .await?
    .into_iter()
    .filter(|(_, product_id)| !archived.contains(product_id))
    .collect();
    for (id, counts) in category_counts(links, &memberships) {
        if stored.get(id.as_str()) == Some(&(counts.product_count, counts.subtree_count)) {
            continue;
        }
        execute(
            conn,
            "UPDATE catalog_categories SET product_count = ?3, subtree_count = ?4
             WHERE tenant_id = ?1 AND id = ?2",
            (
                tenant_id,
                id.as_str(),
                counts.product_count,
                counts.subtree_count,
            ),
        )
        .await?;
    }
    Ok(())
}

/// The live categories of a tenant, each level in its own order, with their
/// stored counts.
pub async fn categories_of_tenant(db: &Db, tenant_id: &str) -> Result<Vec<CategoryRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, slug, name, description, parent_id, position, archived, in_menu,
            created_by, created_at, updated_at, product_count, subtree_count
         FROM catalog_categories c
         WHERE tenant_id = ?1 AND archived = 0
         ORDER BY position, name, id",
        (tenant_id,),
    )
    .await
}

/// One category of a tenant, by id, archived or not.
pub async fn category_by_id(db: &Db, tenant_id: &str, id: &str) -> Result<Option<CategoryRow>> {
    db.fetch_optional(
        "SELECT tenant_id, id, slug, name, description, parent_id, position, archived, in_menu,
            created_by, created_at, updated_at, product_count, subtree_count
         FROM catalog_categories c
         WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, id),
    )
    .await
}

/// How many live categories sit directly under this one.
pub async fn live_child_count(db: &Db, tenant_id: &str, id: &str) -> Result<i64> {
    db.fetch_scalar(
        "SELECT count(*) FROM catalog_categories
         WHERE tenant_id = ?1 AND parent_id = ?2 AND archived = 0",
        (tenant_id, id),
    )
    .await
}

/// The categories a product is in. Live ones come first, in tree order;
/// archived memberships come last, so a page can show them as such.
pub async fn categories_of_product(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
) -> Result<Vec<CategoryRow>> {
    db.fetch_all(
        "SELECT c.tenant_id, c.id, c.slug, c.name, c.description, c.parent_id, c.position,
            c.archived, c.in_menu, c.created_by, c.created_at, c.updated_at, c.product_count,
            c.subtree_count
         FROM catalog_categories c
         JOIN catalog_product_categories pc
            ON pc.tenant_id = c.tenant_id AND pc.category_id = c.id
         WHERE c.tenant_id = ?1 AND pc.product_id = ?2
         ORDER BY c.archived, c.position, c.name, c.id",
        (tenant_id, product_id),
    )
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
