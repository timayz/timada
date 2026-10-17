//! A product: its pictures, its price, the options to pick, the facts about
//! it, what shoppers said (paged with `?reviews=`), and what goes with it.
//! `?error=` carries a refusal from the cart.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;

use crate::actions::query;
use crate::context::{
    categories, category_path, product_categories, related_products, require_product,
};
use crate::query::{CategoryNode, Product, Variant, menu_of};
use crate::reviews::{ReviewPage, reviews_of};
use crate::storefront::{money_format, paths};

use super::{Crumb, Flash, crumb, flash, neighbours};

#[derive(serde::Deserialize)]
struct ProductPageQuery {
    error: Option<String>,
    reviews: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductOptions {
    /// How many related products to show.
    pub related: u32,
}

impl Default for ProductOptions {
    fn default() -> Self {
        Self { related: 4 }
    }
}

/// A variant with its price written the shop's way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantPrice {
    pub variant: Variant,
    pub price: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductModel {
    pub nav: Vec<CategoryNode>,
    pub product: Product,
    /// The price line: one amount, or a range across the variants.
    pub price: String,
    /// Whether the price depends on what is picked.
    pub varies: bool,
    pub currency: String,
    /// The average rating to one decimal, written with the shop's decimal
    /// mark (`4,5`), once reviewed.
    pub rating: Option<String>,
    /// The first paragraph of the description, and the rest.
    pub intro: String,
    pub more: String,
    /// The first category it is filed under and the categories above it.
    pub crumbs: Vec<Crumb>,
    pub reviews: ReviewPage,
    pub reviews_prev_url: Option<String>,
    pub reviews_next_url: Option<String>,
    pub related: Vec<Product>,
    pub variants: Vec<VariantPrice>,
    pub error: Option<Flash>,
    pub url: String,
    pub home_url: String,
}

/// 404 when the slug is no published product.
pub async fn product_page(cx: &Cx, slug: &str, options: &ProductOptions) -> Result<ProductModel> {
    let query: ProductPageQuery = query(cx)?;
    let product = require_product(cx, slug).await?;
    let reviews = reviews_of(cx, &product, query.reviews.unwrap_or(1)).await?;
    let tree = categories(cx).await?;
    let nav = menu_of(&tree);
    let related = related_products(cx, &product, options.related).await?;
    let crumbs = match product_categories(cx, &product).await?.into_iter().next() {
        Some(category) => {
            let mut path = category_path(&tree, &category);
            path.push(category);
            path.iter().map(|category| crumb(cx, category)).collect()
        }
        None => Vec::new(),
    };
    let money = money_format(cx);
    let paths = paths(cx);
    let varies = product.price_range().is_some_and(|range| range.varies());
    let currency = product.currency.clone().unwrap_or_default();
    // The first paragraph introduces the product beside its price; the
    // rest, when there is more, folds under "Description".
    let (intro, more) = match product.description.split_once("\n\n") {
        Some((intro, more)) => (intro.trim().to_owned(), more.trim().to_owned()),
        None => (product.description.trim().to_owned(), String::new()),
    };
    let (reviews_prev_url, reviews_next_url) =
        neighbours(reviews.page, reviews.pages(), |number| {
            paths.product_reviews_url(&product.slug, number)
        });
    let variants = product
        .variants
        .iter()
        .map(|variant| VariantPrice {
            variant: variant.clone(),
            price: money.format(variant.amount_cents.max(0) as u64, &variant.currency),
        })
        .collect();
    Ok(ProductModel {
        nav,
        price: money.price_line(&product),
        varies,
        currency,
        rating: product
            .rating()
            .map(|rating| format!("{rating:.1}").replace('.', &money.decimal.to_string())),
        intro,
        more,
        crumbs,
        reviews,
        reviews_prev_url,
        reviews_next_url,
        related,
        variants,
        error: flash(cx, query.error.as_deref()),
        url: paths.product_url(&product.slug, None),
        home_url: paths.home_url(),
        product,
    })
}
