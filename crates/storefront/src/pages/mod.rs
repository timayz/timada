//! What each page shows, worked out for it: one async function per page
//! returns a model with every value the markup needs (the data, the amounts
//! written the shop's way, the links, the wording of a refusal), and does
//! the page's redirects and 404s. The app's `#[page]` is then the markup
//! around the model.
//!
//! ```ignore
//! #[page]
//! pub async fn index(cx: &Cx) -> Result<impl View> {
//!     let page = pages::product_page(cx, path_param::<Slug>(cx), &ProductOptions::default()).await?;
//!     Ok(view! { document(nav: &page.nav, <h1>(&page.product.name)</h1> ...) })
//! }
//! ```

mod account;
mod cart;
mod catalog;
mod category;
mod checkout;
mod drawer;
mod home;
mod order;
mod product;
mod shell;
mod sign_in;

pub use account::*;
pub use cart::*;
pub use catalog::*;
pub use category::*;
pub use checkout::*;
pub use drawer::*;
pub use home::*;
pub use order::*;
pub use product::*;
pub use shell::*;
pub use sign_in::*;

use topcoat_core::context::Cx;

use crate::cart::CartLineView;
use crate::query::{Category, CategoryNode};
use crate::storefront::{messages, money_format, paths};

/// A refusal to show, by code and in the shop's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flash {
    pub code: String,
    pub text: String,
}

pub(crate) fn flash(cx: &Cx, code: Option<&str>) -> Option<Flash> {
    code.map(|code| Flash {
        code: code.to_owned(),
        text: messages(cx).say(code).to_owned(),
    })
}

/// A link in a breadcrumb, or to a category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub name: String,
    pub slug: String,
    pub url: String,
}

pub(crate) fn crumb(cx: &Cx, category: &Category) -> Crumb {
    Crumb {
        name: category.name.clone(),
        slug: category.slug.clone(),
        url: paths(cx).category_link(&category.slug),
    }
}

/// A category under another, as a listing's chips show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubCategory {
    pub category: Category,
    pub product_count: i64,
    pub url: String,
}

pub(crate) fn sub_category(cx: &Cx, node: &CategoryNode) -> SubCategory {
    SubCategory {
        category: node.category.clone(),
        product_count: node.product_count,
        url: paths(cx).category_link(&node.category.slug),
    }
}

/// How many products a listing shows at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOptions {
    pub per_page: u32,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self { per_page: 36 }
    }
}

/// One line of a cart, with its amounts written the shop's way and its
/// product's page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineModel {
    pub line: CartLineView,
    /// The product's name, or what the shop calls one no longer sold.
    pub name: String,
    /// The product's page, while it is on sale.
    pub url: Option<String>,
    /// Whether it can still be bought.
    pub available: bool,
    /// The price of one.
    pub unit: Option<String>,
    /// What the whole line costs.
    pub amount: Option<String>,
    /// What the one unit an offer prices costs, when one does.
    pub offer_unit: Option<String>,
}

pub(crate) fn line_models<'a>(
    cx: &Cx,
    lines: impl IntoIterator<Item = &'a CartLineView>,
    currency: &str,
) -> Vec<LineModel> {
    let money = money_format(cx);
    let paths = paths(cx);
    let off_sale = messages(cx).off_sale_product.to_string();
    lines
        .into_iter()
        .map(|line| LineModel {
            name: if line.name.is_empty() {
                off_sale.clone()
            } else {
                line.name.clone()
            },
            url: line
                .slug
                .as_deref()
                .map(|slug| paths.product_url(slug, None)),
            available: line.amount_cents.is_some(),
            unit: line
                .unit_amount_cents
                .map(|cents| money.format(cents, currency)),
            amount: line.amount_cents.map(|cents| money.format(cents, currency)),
            offer_unit: line
                .offer
                .as_ref()
                .map(|offer| money.format(offer.unit_amount_cents, currency)),
            line: line.clone(),
        })
        .collect()
}

/// `Ada L.` as `AL`: an avatar for a review.
pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// The pages before and after `page` of `pages`, as `link` writes them.
pub(crate) fn neighbours(
    page: u32,
    pages: u32,
    link: impl Fn(u32) -> String,
) -> (Option<String>, Option<String>) {
    let prev = (page > 1).then(|| link(page - 1));
    let next = (page < pages).then(|| link(page + 1));
    (prev, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_two_words() {
        assert_eq!(initials("Ada L."), "AL");
        assert_eq!(initials("ada"), "A");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn neighbours_stop_at_the_ends() {
        let link = |n: u32| format!("?page={n}");
        assert_eq!(neighbours(1, 3, link), (None, Some("?page=2".into())));
        assert_eq!(neighbours(3, 3, link), (Some("?page=2".into()), None));
        assert_eq!(neighbours(1, 1, link), (None, None));
    }
}
