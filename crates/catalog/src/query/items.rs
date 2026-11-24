//! Which product a feed's item is, and which categories it is filed under:
//! what a report keyed by the feed's item ids (an ad network's) is read
//! back into the catalog with.
//!
//! An item of the feed is a product, or one variant of a product sold in
//! several; either way its id is the catalog's own. Ad networks report
//! the ids in lower case, so they are matched that way.

use std::collections::HashMap;

use anyhow::Result;
use timada_core::db::Db;

/// A shop's catalog, as far as telling what a feed's item is needs it.
#[derive(Debug, Clone, Default)]
pub struct FeedItems {
    /// By a product's id in lower case: its id and its name.
    products: HashMap<String, (String, String)>,
    /// By a variant's id in lower case: its product's id.
    variants: HashMap<String, String>,
    /// By a product's id: the ids of the categories it is filed under.
    filed: HashMap<String, Vec<String>>,
    /// By a category's id: its name.
    categories: HashMap<String, String>,
}

impl FeedItems {
    /// The product an item of the feed is, `(id, name)`: the item is the
    /// product itself, or one of its variants. `None` for an item the
    /// catalog no longer has.
    pub fn product_of(&self, item_id: &str) -> Option<(&str, &str)> {
        let item_id = item_id.to_lowercase();
        let product = match self.variants.get(&item_id) {
            Some(product_id) => self.products.get(&product_id.to_lowercase()),
            None => self.products.get(&item_id),
        }?;
        Some((product.0.as_str(), product.1.as_str()))
    }

    /// The categories a product is filed under, `(id, name)`, in no order.
    pub fn categories_of(&self, product_id: &str) -> Vec<(&str, &str)> {
        self.filed
            .get(product_id)
            .into_iter()
            .flatten()
            .filter_map(|id| Some((id.as_str(), self.categories.get(id)?.as_str())))
            .collect()
    }
}

/// The statements of [`feed_items`]: each reads one tenant's rows of a
/// table off its key, and they are put together here rather than joined
/// (a join through part of a key reads the other table once a row).
pub(super) const FEED_ITEM_STATEMENTS: [&str; 4] = [
    "SELECT id, name FROM catalog_products WHERE tenant_id = ?1",
    "SELECT id, product_id FROM catalog_variants WHERE tenant_id = ?1",
    "SELECT product_id, category_id FROM catalog_product_categories WHERE tenant_id = ?1",
    "SELECT id, name FROM catalog_categories WHERE tenant_id = ?1",
];

/// What telling a shop's feed items apart needs of its catalog. It reads
/// every product of the shop once: for a report, not for a page a shopper
/// waits on.
pub async fn feed_items(db: &Db, tenant_id: &str) -> Result<FeedItems> {
    let read = |statement: &'static str| async move {
        db.fetch_all::<(String, String)>(statement, (tenant_id,))
            .await
    };
    let mut items = FeedItems::default();
    for (id, name) in read(FEED_ITEM_STATEMENTS[0]).await? {
        items.products.insert(id.to_lowercase(), (id, name));
    }
    for (id, product_id) in read(FEED_ITEM_STATEMENTS[1]).await? {
        items.variants.insert(id.to_lowercase(), product_id);
    }
    for (product_id, category_id) in read(FEED_ITEM_STATEMENTS[2]).await? {
        items.filed.entry(product_id).or_default().push(category_id);
    }
    items.categories = read(FEED_ITEM_STATEMENTS[3]).await?.into_iter().collect();
    Ok(items)
}
