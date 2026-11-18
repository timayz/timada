//! What the shop sells, found the way a crawler would: the categories its
//! menu links to and the products their pages list. Plus the few things a
//! shopper reads off a page (the picker's variants, the cart's lines, the
//! checkout's totals), by pattern rather than with an HTML parser: the
//! markup is the storefront's own and stable.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use anyhow::{Result, ensure};
use regex::Regex;

use crate::http::Visitor;

/// Category pages read per category while looking for products.
const CRAWL_PAGES: u32 = 3;

pub struct Catalog {
    /// The path of each category's page (`/jeux-pour-chat/souris`).
    pub categories: Vec<String>,
    pub products: Vec<Listed>,
    /// Words shoppers search for, taken from the products' slugs.
    pub terms: Vec<String>,
}

/// A product a page lists: where its card leads, and the slug the cart's
/// form names it by.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Listed {
    pub path: String,
    pub slug: String,
}

impl Catalog {
    pub async fn discover(visitor: &mut Visitor<'_>) -> Result<Self> {
        let home = visitor.get("/").await?;
        let mut categories = category_links(&home.body);
        // By slug: a product listed under two categories is one product,
        // at the address the first page that listed it gave.
        let mut products: BTreeMap<String, String> = product_links(&home.body);
        for category in categories.clone() {
            for page in 1..=CRAWL_PAGES {
                let found = visitor.get(&format!("{category}?page={page}")).await?;
                let before = products.len();
                for (slug, path) in product_links(&found.body) {
                    products.entry(slug).or_insert(path);
                }
                categories.extend(category_links(&found.body));
                if products.len() == before {
                    break;
                }
            }
        }
        let terms: BTreeSet<String> = products
            .keys()
            .flat_map(|slug| slug.split('-'))
            .filter(|word| word.len() >= 4)
            .map(str::to_owned)
            .collect();
        ensure!(
            !products.is_empty(),
            "the shop lists no products: is it seeded and is the storefront caught up?"
        );
        Ok(Self {
            categories: categories.into_iter().collect(),
            products: products
                .into_iter()
                .map(|(slug, path)| Listed { path, slug })
                .collect(),
            terms: terms.into_iter().collect(),
        })
    }
}

/// A link to a category's page: the menu's, a page's chips of
/// sub-categories, the home's tiles. Its address is the category's path.
static CATEGORY_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"data-(?:category|subcategory|tile)="[^"]+"[^>]*?href="(/[^"?#]+)""#).unwrap()
});
/// A product's card: its slug, then where its picture leads.
static PRODUCT_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"data-product="([^"]+)"[^>]*>\s*<a class="card-pic" href="(/[^"?#]+)""#).unwrap()
});

/// The paths of the categories `body` links to.
fn category_links(body: &str) -> BTreeSet<String> {
    CATEGORY_LINK
        .captures_iter(body)
        .map(|captures| captures[1].to_owned())
        .collect()
}

/// The products `body` lists, by slug, each with where its card leads.
fn product_links(body: &str) -> BTreeMap<String, String> {
    PRODUCT_LINK
        .captures_iter(body)
        .map(|captures| (captures[1].to_owned(), captures[2].to_owned()))
        .collect()
}

/// The variants the product page's table offers, those left to sell.
pub fn variants_on_sale(body: &str) -> Vec<String> {
    static ROW: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<tr\s[^>]*data-variant[^>]*>").unwrap());
    static ID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"data-variant="?([^"\s>]+)"#).unwrap());
    ROW.find_iter(body)
        .filter(|row| !sold_out(row.as_str()))
        .filter_map(|row| ID.captures(row.as_str()).map(|c| c[1].to_owned()))
        .collect()
}

/// Whether the page says the product itself cannot be bought.
pub fn product_sold_out(body: &str) -> bool {
    body.contains(r#"data-stock="sold-out""#)
}

fn sold_out(tag: &str) -> bool {
    tag.contains("data-sold-out") && !tag.contains(r#"data-sold-out="false""#)
}

/// A line of the cart page: what its forms send back.
pub struct CartLine {
    pub product_id: String,
    pub variant_id: String,
    pub quantity: u32,
}

pub fn cart_lines(body: &str) -> Vec<CartLine> {
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r#"name="product_id" value="([^"]*)">\s*"#,
            r#"<input type="hidden" name="variant_id" value="([^"]*)">"#,
            r#"[\s\S]*?name="quantity"[^>]*value="(\d+)""#,
        ))
        .unwrap()
    });
    LINE.captures_iter(body)
        .map(|c| CartLine {
            product_id: c[1].to_owned(),
            variant_id: c[2].to_owned(),
            quantity: c[3].parse().unwrap_or(1),
        })
        .collect()
}

/// What the checkout form holds before the shopper fills it.
pub struct CheckoutForm {
    /// The goods, before delivery.
    pub goods_cents: u64,
    /// The delivery methods: id, price, countries.
    pub methods: Vec<(String, u64, Vec<String>)>,
}

pub fn checkout_form(body: &str) -> Option<CheckoutForm> {
    static GOODS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"data-checkout-form[^>]*data-goods="(\d+)""#).unwrap());
    static METHOD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"<input type="radio" name="shipping_method"[^>]*>"#).unwrap()
    });
    let attribute = |tag: &str, name: &str| {
        Regex::new(&format!(r#"{name}="([^"]*)""#))
            .ok()?
            .captures(tag)
            .map(|c| c[1].to_owned())
    };
    let goods_cents = GOODS.captures(body)?[1].parse().ok()?;
    let methods = METHOD
        .find_iter(body)
        .filter_map(|tag| {
            let tag = tag.as_str();
            Some((
                attribute(tag, "value")?,
                attribute(tag, "data-amount")?.parse().ok()?,
                attribute(tag, "data-countries")
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
            ))
        })
        .collect();
    Some(CheckoutForm {
        goods_cents,
        methods,
    })
}

/// The `error` an address carries, as the shop's refusals name it.
pub fn error_of(path: &str) -> Option<&str> {
    let query = path.split_once('?')?.1;
    query
        .split(['&', '#'])
        .find_map(|pair| pair.strip_prefix("error="))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_left_to_sell_are_read_off_the_table() {
        let body = r#"<tr data-variant="A1" data-price="900" data-sold-out="false"><td>
            <tr data-variant="B2" data-price="900" data-sold-out=""><td>
            <tr data-variant="C3" data-price="900"><td>"#;
        assert_eq!(variants_on_sale(body), vec!["A1", "C3"]);
    }

    #[test]
    fn the_cart_lines_are_read_off_their_forms() {
        let body = r#"<form method="post" action="/cart/quantity">
            <input type="hidden" name="product_id" value="P1">
            <input type="hidden" name="variant_id" value="">
            <label><span hidden="">x</span>
            <input type="number" name="quantity" min="0" max="99" value="2" aria-label="q"></label>"#;
        let lines = cart_lines(body);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            (
                lines[0].product_id.as_str(),
                lines[0].variant_id.as_str(),
                lines[0].quantity
            ),
            ("P1", "", 2)
        );
    }

    #[test]
    fn the_checkout_totals_are_read_off_the_form() {
        let body = r#"<form class="checkout" method="post" action="/checkout" data-checkout-form=""
            data-start-path="/checkout/start" data-amount="2990" data-goods="2500" data-currency="eur">
            <input type="radio" name="shipping_method" value="M1" data-amount="490" data-price="4,90 €" data-countries="FR BE CH" checked="">"#;
        let form = checkout_form(body).unwrap();
        assert_eq!(form.goods_cents, 2500);
        assert_eq!(
            form.methods,
            vec![(
                "M1".into(),
                490,
                vec!["FR".into(), "BE".into(), "CH".into()]
            )]
        );
    }

    #[test]
    fn categories_and_products_are_found_by_their_links() {
        let body = r#"<a class="menu-top" data-category="souris" data-products="2" href="/jeux/souris">Souris</a>
            <li class="card" data-product="peluche">
            <a class="card-pic" href="/jeux/souris/peluche"><img></a></li>
            <li class="card" data-product="balle"><a class="card-pic" href="/p/balle"></a></li>"#;
        assert_eq!(
            category_links(body).into_iter().collect::<Vec<_>>(),
            ["/jeux/souris"]
        );
        assert_eq!(
            product_links(body).into_iter().collect::<Vec<_>>(),
            [
                ("balle".to_owned(), "/p/balle".to_owned()),
                ("peluche".to_owned(), "/jeux/souris/peluche".to_owned()),
            ]
        );
    }

    #[test]
    fn a_refusal_is_read_off_the_address() {
        assert_eq!(
            error_of("/p/x?error=out_of_stock#cart"),
            Some("out_of_stock")
        );
        assert_eq!(error_of("/p/x#cart"), None);
    }
}
