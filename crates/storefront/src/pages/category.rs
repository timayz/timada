//! A category and the products in it, sub-categories included, sorted with
//! `?sort=` and paged with `?page=`.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;

use crate::actions::query;
use crate::context::{categories, category_path, products, require_category_in};
use crate::query::{Category, CategoryNode, ProductPage, ProductQuery, ProductSort, menu_of};
use crate::storefront::paths;

use super::{Crumb, ListOptions, SubCategory, crumb, neighbours, sub_category};

#[derive(serde::Deserialize)]
struct CategoryQuery {
    sort: Option<String>,
    page: Option<u32>,
}

/// The sorts a listing offers, in the order a menu shows them.
pub const SORTS: &[ProductSort] = &[
    ProductSort::Newest,
    ProductSort::TopRated,
    ProductSort::PriceAsc,
    ProductSort::PriceDesc,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryModel {
    pub nav: Vec<CategoryNode>,
    pub category: Category,
    pub children: Vec<SubCategory>,
    /// The categories above, top-level first.
    pub crumbs: Vec<Crumb>,
    pub sort: ProductSort,
    pub products: ProductPage,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    /// This page, unsorted.
    pub url: String,
    pub home_url: String,
}

impl CategoryModel {
    /// The sorts to offer, with whether each is the current one.
    pub fn sorts(&self) -> impl Iterator<Item = (ProductSort, bool)> + '_ {
        SORTS.iter().map(move |sort| (*sort, *sort == self.sort))
    }
}

/// 404 when the slug is no live category.
pub async fn category_page(cx: &Cx, slug: &str, options: &ListOptions) -> Result<CategoryModel> {
    // The tree holds every live category, so the page's own comes out of it
    // even when the merchant left it out of the menu.
    let tree = categories(cx).await?;
    let nav = menu_of(&tree);
    let node = require_category_in(&tree, slug)?;
    let category = node.category.clone();
    let children = node
        .children
        .iter()
        .map(|child| sub_category(cx, child))
        .collect();
    let crumbs = category_path(&tree, &category)
        .iter()
        .map(|ancestor| crumb(cx, ancestor))
        .collect();
    let query: CategoryQuery = query(cx)?;
    let sort = query
        .sort
        .as_deref()
        .map(ProductSort::parse)
        .unwrap_or_default();
    // The products of this category and of everything below it, which is
    // what a shopper expects from a top-level entry.
    let page = products(
        cx,
        &ProductQuery::default()
            .in_category(category.id.clone())
            .sort(sort)
            .per_page(options.per_page)
            .page(query.page.unwrap_or(1)),
    )
    .await?;
    let paths = paths(cx);
    let (prev_url, next_url) = neighbours(page.page, page.pages(), |number| {
        paths.category_url(&category.slug, sort, number)
    });
    Ok(CategoryModel {
        nav,
        url: paths.category_link(&category.slug),
        category,
        children,
        crumbs,
        sort,
        products: page,
        prev_url,
        next_url,
        home_url: paths.home_url(),
    })
}
