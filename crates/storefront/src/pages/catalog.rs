//! The whole catalogue, newest first, searched with `?q=` and paged with
//! `?page=`.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;

use crate::actions::query;
use crate::context::{menu, products};
use crate::query::{CategoryNode, ProductPage, ProductQuery, ProductSort};
use crate::storefront::paths;

use super::{ListOptions, neighbours};

#[derive(serde::Deserialize)]
struct CatalogQuery {
    q: Option<String>,
    page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    pub nav: Vec<CategoryNode>,
    /// What was searched for, trimmed; `None` for the whole catalogue.
    pub q: Option<String>,
    pub products: ProductPage,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    pub home_url: String,
}

pub async fn catalog_page(cx: &Cx, options: &ListOptions) -> Result<CatalogModel> {
    let query: CatalogQuery = query(cx)?;
    let q = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_owned);
    let mut wanted = ProductQuery::default()
        .page(query.page.unwrap_or(1))
        .per_page(options.per_page)
        .sort(ProductSort::Newest);
    if let Some(q) = &q {
        wanted = wanted.search(q.as_str());
    }
    let page = products(cx, &wanted).await?;
    let nav = menu(cx).await?;
    let paths = paths(cx);
    let (prev_url, next_url) = neighbours(page.page, page.pages(), |number| {
        paths.catalog_url(q.as_deref(), number)
    });
    Ok(CatalogModel {
        nav,
        q,
        products: page,
        prev_url,
        next_url,
        home_url: paths.home_url(),
    })
}
