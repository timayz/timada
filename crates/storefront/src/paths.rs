//! Where the shop's categories and products are: a category's page at the
//! root, by its full path in the tree (`/jeux-pour-chat/souris`), and a
//! product's under the path of a category it is filed under, then its slug
//! (`/jeux-pour-chat/souris/souris-en-peluche`).
//!
//! A product filed under several categories has an address under each: a
//! listing links it under the category whose page it is, and everything
//! else (a search, the cart, the sitemap, a feed) under its main one, which
//! is also the address search engines are told. A product filed nowhere
//! has none of these: where it is then is the host's.
//!
//! The library gives the paths ([`paths`]) and reads one back
//! ([`resolve_path`]); which route answers is the host's, tried after its
//! own fixed ones. In a request read in another language
//! ([`read_in`](crate::read_in)) each part of a path is the slug of that
//! language.

use std::collections::HashMap;

use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::{categories, category_by_slug, product_by_slug, store};
use crate::query::{CategoryNode, Product, published_categories};

/// The path of every live category, by its id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paths {
    by_id: HashMap<String, String>,
    by_path: HashMap<String, String>,
    by_slug: HashMap<String, String>,
}

impl Paths {
    /// The paths of the categories of `nodes`, the whole tree.
    pub fn of(nodes: &[CategoryNode]) -> Self {
        fn walk(paths: &mut Paths, nodes: &[CategoryNode], above: &str) {
            for node in nodes {
                let path = format!("{above}/{}", node.category.slug);
                paths.by_id.insert(node.category.id.clone(), path.clone());
                paths.by_path.insert(path.clone(), node.category.id.clone());
                paths
                    .by_slug
                    .insert(node.category.slug.clone(), node.category.id.clone());
                walk(paths, &node.children, &path);
            }
        }
        let mut paths = Paths::default();
        walk(&mut paths, nodes, "");
        paths
    }

    /// Where a category's page is (`/jeux-pour-chat/souris`); `None` for
    /// one that is not live.
    pub fn category(&self, category_id: &str) -> Option<&str> {
        self.by_id.get(category_id).map(String::as_str)
    }

    /// The id of the category whose page is at `path`.
    pub fn at(&self, path: &str) -> Option<&str> {
        self.by_path.get(path).map(String::as_str)
    }

    /// The id of the category that goes by `slug`.
    pub fn by_slug(&self, slug: &str) -> Option<&str> {
        self.by_slug.get(slug).map(String::as_str)
    }

    /// A product's address under one category: that category's path, then
    /// the product's slug. `None` for a category that is not live.
    pub fn under(&self, category_id: &str, slug: &str) -> Option<String> {
        self.category(category_id)
            .map(|path| format!("{path}/{slug}"))
    }

    /// The main category among `categories`, a product's
    /// ([`Product::categories`]): the first of them that is live.
    pub fn main<'a>(&self, categories: &'a [String]) -> Option<&'a str> {
        categories
            .iter()
            .map(String::as_str)
            .find(|id| self.by_id.contains_key(*id))
    }

    /// The default address of a product filed under `categories` that
    /// goes by `slug`: under its main category. `None` for a product
    /// filed under no live category.
    pub fn filed(&self, categories: &[String], slug: &str) -> Option<String> {
        self.under(self.main(categories)?, slug)
    }

    /// The default address of `product`: see [`filed`](Self::filed).
    pub fn product(&self, product: &Product) -> Option<String> {
        self.filed(&product.categories, &product.slug)
    }
}

/// The paths of the shop's categories, in the language of the request.
/// It reads the tree: a host that asks more than once in a request keeps
/// the answer (a page that already has the tree builds them with
/// [`Paths::of`]).
pub async fn paths(cx: &Cx) -> Result<Paths> {
    Ok(Paths::of(&categories(cx).await?))
}

/// The paths of the shop's categories as `language` reads them, whatever
/// the language of the request; `None` is the shop's main language. What a
/// page names its other languages' addresses from.
pub async fn paths_in(cx: &Cx, language: Option<&str>) -> Result<Paths> {
    let db = store(cx).db();
    let read = async {
        let mut nodes = published_categories(db).await?;
        if let Some(language) = language {
            crate::read_in::categories_in(db, language, &mut nodes).await?;
        }
        anyhow::Ok(Paths::of(&nodes))
    };
    read.await.map_err(Error::from_anyhow)
}

/// What a path of the shop names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addressed {
    /// The page of the category that goes by this slug.
    Category(String),
    /// The page of the product that goes by `slug`, read under the
    /// category `category_id`, one of those it is filed under.
    Product { slug: String, category_id: String },
    /// Something that is at another path, to answer with a permanent
    /// redirect: a category asked under a wrong parent, a product under a
    /// category it is not filed under or with no category at all, either
    /// by the slug of another language.
    Moved(String),
    /// The product that goes by this slug, filed under no live category:
    /// it has no path, and where it is is the host's.
    Unfiled(String),
}

/// What `path` names, if anything: its last part is a category's slug or a
/// product's, the ones before it the category's ancestors, or the path of
/// a category the product is filed under. In the language of the request,
/// as `paths` are.
pub async fn resolve_path(cx: &Cx, paths: &Paths, path: &str) -> Result<Option<Addressed>> {
    let trimmed = path.trim_matches('/');
    let Some(last) = trimmed.rsplit('/').next().filter(|last| !last.is_empty()) else {
        return Ok(None);
    };
    let path = format!("/{trimmed}");
    let above = &path[..path.len() - last.len() - 1];

    // A category, by the slug it goes by here or, in another language, by
    // its main one where it has its own.
    let category_id = match paths.by_slug(last) {
        Some(id) => Some(id.to_owned()),
        None => category_by_slug(cx, last)
            .await?
            .map(|category| category.id),
    };
    if let Some(own) = category_id.as_deref().and_then(|id| paths.category(id)) {
        return Ok(Some(if own == path {
            Addressed::Category(last.to_owned())
        } else {
            Addressed::Moved(own.to_owned())
        }));
    }

    let Some(product) = product_by_slug(cx, last).await? else {
        return Ok(None);
    };
    if product.slug == last
        && let Some(category_id) = paths.at(above)
        && product.categories.iter().any(|id| id == category_id)
    {
        return Ok(Some(Addressed::Product {
            slug: product.slug,
            category_id: category_id.to_owned(),
        }));
    }
    Ok(Some(match paths.product(&product) {
        Some(default) => Addressed::Moved(default),
        None => Addressed::Unfiled(product.slug),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Category;

    fn node(id: &str, children: Vec<CategoryNode>) -> CategoryNode {
        CategoryNode {
            category: Category {
                id: id.to_owned(),
                slug: id.to_owned(),
                name: id.to_owned(),
                description: String::new(),
                parent_id: None,
                position: 0,
                in_menu: true,
            },
            children,
            product_count: 0,
        }
    }

    #[test]
    fn a_category_is_at_its_path_and_a_product_under_its_main_one() {
        let paths = Paths::of(&[
            node("jeux", vec![node("souris", Vec::new())]),
            node("couchage", Vec::new()),
        ]);
        assert_eq!(paths.category("souris"), Some("/jeux/souris"));
        assert_eq!(paths.category("couchage"), Some("/couchage"));
        assert_eq!(paths.at("/jeux/souris"), Some("souris"));
        assert_eq!(paths.at("/souris"), None);
        assert_eq!(paths.by_slug("souris"), Some("souris"));

        let filed = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        // Under its main category, the first of the list that is live.
        assert_eq!(
            paths.filed(&filed(&["souris", "couchage"]), "peluche"),
            Some("/jeux/souris/peluche".to_owned())
        );
        assert_eq!(
            paths.filed(&filed(&["archived", "couchage"]), "peluche"),
            Some("/couchage/peluche".to_owned())
        );
        assert_eq!(paths.filed(&filed(&["archived"]), "peluche"), None);
        assert_eq!(paths.filed(&[], "peluche"), None);
        assert_eq!(
            paths.under("couchage", "peluche"),
            Some("/couchage/peluche".to_owned())
        );
    }
}
