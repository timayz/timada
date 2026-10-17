//! The home page: the newest products, a pick of the best rated, a tile per
//! top category, and the latest reviews.

use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::{menu, products, store};
use crate::query::{CategoryNode, Image, ProductQuery, ProductSort, published_category_pictures};
use crate::reviews::{Review, recent_reviews};
use crate::storefront::paths;

/// How many of each the home shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeOptions {
    pub newest: u32,
    pub picks: u32,
    pub tiles: usize,
    pub quotes: u32,
}

impl Default for HomeOptions {
    fn default() -> Self {
        Self {
            newest: 8,
            picks: 6,
            tiles: 8,
            quotes: 3,
        }
    }
}

/// A top category's tile: the category, its count, the picture of its
/// newest product with one, and its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryTile {
    pub node: CategoryNode,
    pub image: Option<Image>,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeModel {
    pub nav: Vec<CategoryNode>,
    pub newest: Vec<crate::query::Product>,
    /// The best rated products that are not among the newest.
    pub picks: Vec<crate::query::Product>,
    pub tiles: Vec<CategoryTile>,
    pub quotes: Vec<Review>,
    /// Where "discover" goes: the first category of the menu, else the
    /// catalogue.
    pub discover_url: String,
    pub catalog_url: String,
}

pub async fn home_page(cx: &Cx, options: &HomeOptions) -> Result<HomeModel> {
    let nav = menu(cx).await?;
    let paths = paths(cx);
    let newest = products(
        cx,
        &ProductQuery::default()
            .sort(ProductSort::Newest)
            .per_page(options.newest.max(1)),
    )
    .await?
    .items;
    let picks = products(
        cx,
        &ProductQuery::default()
            .sort(ProductSort::TopRated)
            .exclude_all(newest.iter().map(|p| p.id.clone()))
            .per_page(options.picks.max(1)),
    )
    .await?
    .items;
    let roots: Vec<&CategoryNode> = nav.iter().take(options.tiles).collect();
    let ids: Vec<String> = roots.iter().map(|node| node.category.id.clone()).collect();
    let store = store(cx);
    let mut pictures = published_category_pictures(store.pool(), &ids)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))?;
    let tiles = roots
        .into_iter()
        .map(|node| {
            let image = pictures.remove(&node.category.id).map(|mut image| {
                image.url = match store.media_url() {
                    Some(base) => format!("{base}/{}", image.file),
                    None => image.file.clone(),
                };
                image
            });
            CategoryTile {
                url: paths.category_link(&node.category.slug),
                node: node.clone(),
                image,
            }
        })
        .collect();
    let quotes = recent_reviews(cx, options.quotes.max(1)).await?;
    let discover_url = nav
        .first()
        .map(|node| paths.category_link(&node.category.slug))
        .unwrap_or_else(|| paths.catalog_url(None, 1));
    Ok(HomeModel {
        nav,
        newest,
        picks,
        tiles,
        quotes,
        discover_url,
        catalog_url: paths.catalog_url(None, 1),
    })
}
