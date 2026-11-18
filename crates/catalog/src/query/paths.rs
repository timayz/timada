//! Where a storefront reads the shop's categories and products, worked out
//! from the back office's own rows: for the addresses written outside a
//! request (a feed, a mail). A category's page is at its path in the tree
//! (`/jeux-pour-chat/souris`); a product's default address is under its
//! main category, then its slug, and `/p/{slug}` for one filed nowhere.
//! The storefront library works the same paths out of its own copy.

use std::collections::HashMap;

use anyhow::Result;
use timada_core::db::{Db, Json};

/// The path of every live category of a shop, by its id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategoryPaths {
    by_id: HashMap<String, String>,
}

impl CategoryPaths {
    /// The paths of the categories `rows` list: an id, its slug and the
    /// category above it. One whose chain of parents leaves the rows (an
    /// archived parent's) has no path.
    pub fn of(rows: &[(String, String, Option<String>)]) -> Self {
        let known: HashMap<&str, (&str, Option<&str>)> = rows
            .iter()
            .map(|(id, slug, parent)| (id.as_str(), (slug.as_str(), parent.as_deref())))
            .collect();
        let mut by_id = HashMap::new();
        for (id, _, _) in rows {
            let mut parts = Vec::new();
            let mut at = Some(id.as_str());
            // No deeper than the rows are many: a loop would end there.
            while let Some(current) = at
                && parts.len() <= rows.len()
            {
                let Some((slug, parent)) = known.get(current) else {
                    parts.clear();
                    break;
                };
                parts.push(*slug);
                at = *parent;
            }
            if !parts.is_empty() && at.is_none() {
                parts.reverse();
                by_id.insert(id.clone(), format!("/{}", parts.join("/")));
            }
        }
        CategoryPaths { by_id }
    }

    /// Where a category's page is; `None` for one that is not live.
    pub fn category(&self, category_id: &str) -> Option<&str> {
        self.by_id.get(category_id).map(String::as_str)
    }

    /// The default address of a product that goes by `slug`, filed under
    /// `filed` (its main category first, as
    /// [`ProductState::filed_under`](crate::ProductState::filed_under)
    /// lists them): under the first of them that is live, or `/p/{slug}`
    /// when none is.
    pub fn product(&self, filed: &[String], slug: &str) -> String {
        match filed.iter().find_map(|id| self.category(id)) {
            Some(path) => format!("{path}/{slug}"),
            None => format!("/p/{slug}"),
        }
    }
}

/// The paths of the tenant's live categories.
pub async fn category_paths(db: &Db, tenant_id: &str) -> Result<CategoryPaths> {
    let rows: Vec<(String, String, Option<String>)> = db
        .fetch_all(
            "SELECT id, slug, parent_id FROM catalog_categories
             WHERE tenant_id = ?1 AND archived = 0",
            (tenant_id,),
        )
        .await?;
    Ok(CategoryPaths::of(&rows))
}

/// The categories a product is filed under, its main one first; empty for
/// a product the rows do not hold.
pub async fn filed_under(db: &Db, tenant_id: &str, product_id: &str) -> Result<Vec<String>> {
    let row: Option<(Json<Vec<String>>,)> = db
        .fetch_optional(
            "SELECT filed_json FROM catalog_products WHERE tenant_id = ?1 AND id = ?2",
            (tenant_id, product_id),
        )
        .await?;
    Ok(row.map(|(filed,)| filed.0).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_product_is_under_its_first_live_category() {
        let row = |id: &str, parent: Option<&str>| {
            (id.to_owned(), id.to_owned(), parent.map(str::to_owned))
        };
        let paths = CategoryPaths::of(&[
            row("jeux", None),
            row("souris", Some("jeux")),
            row("couchage", None),
            // Its parent is archived: no page.
            row("orphan", Some("gone")),
        ]);
        assert_eq!(paths.category("souris"), Some("/jeux/souris"));
        assert_eq!(paths.category("orphan"), None);
        let filed = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            paths.product(&filed(&["souris", "couchage"]), "peluche"),
            "/jeux/souris/peluche"
        );
        assert_eq!(
            paths.product(&filed(&["gone", "couchage"]), "peluche"),
            "/couchage/peluche"
        );
        assert_eq!(paths.product(&filed(&["orphan"]), "peluche"), "/p/peluche");
        assert_eq!(paths.product(&[], "peluche"), "/p/peluche");
    }
}
