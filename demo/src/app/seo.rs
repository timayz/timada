//! What the shop tells search engines beyond its pages: schema.org
//! descriptions (JSON-LD), `/sitemap.xml` and `/robots.txt`.

use std::time::{Duration, UNIX_EPOCH};

use timada_catalog::{category_tree, list_categories, listed_products};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::sitemap::{Sitemap, SitemapUrl},
        href, route,
    },
};

use super::{Crumb, absolute, catalog, category};
use crate::Store;

/// A JSON string literal that is also safe inside a `<script>`: `<`, `>` and
/// `&` are written as escapes, so no text can close the element.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' | '>' | '&' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A `BreadcrumbList` for a trail; `here` is the address of the page itself,
/// which the trail's last step does not link.
pub fn breadcrumb_json_ld(trail: &[Crumb], here: &str) -> String {
    let items: Vec<String> = trail
        .iter()
        .enumerate()
        .map(|(index, crumb)| {
            format!(
                r#"{{"@type":"ListItem","position":{},"name":{},"item":{}}}"#,
                index + 1,
                json_string(&crumb.label),
                json_string(&absolute(crumb.link.as_deref().unwrap_or(here)))
            )
        })
        .collect();
    format!(
        r#"{{"@context":"https://schema.org","@type":"BreadcrumbList","itemListElement":[{}]}}"#,
        items.join(",")
    )
}

/// What a product page tells search engines about what it sells.
pub struct ProductOffer<'a> {
    pub name: &'a str,
    pub sku: &'a str,
    pub brand: &'a str,
    pub description: &'a str,
    /// A path of the shop, or a full address.
    pub image: Option<&'a str>,
    /// Minor units and currency; `None` for a product without a price.
    pub price: Option<(i64, &'a str)>,
    pub in_stock: bool,
    /// `(average, count)` of the published reviews, when there are any.
    pub rating: Option<(f64, i64)>,
}

/// A schema.org `Product` with its `Offer` and `AggregateRating`; `here` is
/// the product page's path.
pub fn product_json_ld(product: &ProductOffer<'_>, here: &str) -> String {
    let mut fields = vec![
        r#""@context":"https://schema.org""#.to_owned(),
        r#""@type":"Product""#.to_owned(),
        format!(r#""name":{}"#, json_string(product.name)),
        format!(r#""sku":{}"#, json_string(product.sku)),
        format!(
            r#""brand":{{"@type":"Brand","name":{}}}"#,
            json_string(product.brand)
        ),
    ];
    if !product.description.is_empty() {
        fields.push(format!(
            r#""description":{}"#,
            json_string(product.description)
        ));
    }
    if let Some(image) = product.image {
        let image = if image.starts_with('/') {
            absolute(image)
        } else {
            image.to_owned()
        };
        fields.push(format!(r#""image":{}"#, json_string(&image)));
    }
    if let Some((minor, currency)) = product.price {
        let availability = if product.in_stock {
            "InStock"
        } else {
            "OutOfStock"
        };
        fields.push(format!(
            r#""offers":{{"@type":"Offer","url":{},"price":"{}.{:02}","priceCurrency":{},"availability":"https://schema.org/{availability}"}}"#,
            json_string(&absolute(here)),
            minor / 100,
            minor % 100,
            json_string(currency),
        ));
    }
    if let Some((average, count)) = product.rating.filter(|(_, count)| *count > 0) {
        fields.push(format!(
            r#""aggregateRating":{{"@type":"AggregateRating","ratingValue":"{average:.1}","reviewCount":{count}}}"#
        ));
    }
    format!("{{{}}}", fields.join(","))
}

/// Several descriptions of one page, as a single JSON-LD array.
pub fn json_ld_graph(parts: &[String]) -> String {
    format!("[{}]", parts.join(","))
}

/// The home page, every category the shop shows and every product on sale.
#[route(GET "/sitemap.xml")]
pub async fn sitemap(cx: &Cx) -> Result<Sitemap> {
    let store = app_context::<Store>(cx);
    let mut urls = vec![SitemapUrl::new(absolute(&href!(catalog::home).resolve(cx)))];
    let tree = category_tree(list_categories(&store.db, false).await?, false);
    for (_, shown) in tree.iter().flat_map(|node| node.flatten(0)) {
        let page = href!(category::show, category::CategorySlug(shown.slug.clone())).resolve(cx);
        urls.push(SitemapUrl::new(absolute(&page)));
    }
    for (product_id, updated_at) in listed_products(&store.db).await? {
        let page = href!(catalog::product_page, catalog::ProductId(product_id)).resolve(cx);
        let changed = UNIX_EPOCH + Duration::from_secs(updated_at.max(0) as u64);
        urls.push(SitemapUrl::new(absolute(&page)).last_modified(changed));
    }
    Ok(Sitemap::new().urls(urls))
}

/// Everything public may be crawled; accounts, carts and checkouts are
/// nobody's search result.
#[route(GET "/robots.txt")]
pub async fn robots(cx: &Cx) -> Result<String> {
    Ok(format!(
        "User-agent: *\nDisallow: /account\nDisallow: /cart\nDisallow: /checkout\nDisallow: /admin\n\nSitemap: {}\n",
        absolute(&href!(sitemap).resolve(cx))
    ))
}

#[cfg(test)]
mod tests {
    use super::{ProductOffer, json_ld_graph, json_string, product_json_ld};

    #[test]
    fn nothing_escapes_the_script_element() {
        assert_eq!(
            json_string("</script><b>\"A&B\"\\\n"),
            r#""\u003c/script\u003e\u003cb\u003e\"A\u0026B\"\\\u000a""#
        );
        assert_eq!(json_string("Écrans PC"), "\"Écrans PC\"");
    }

    #[test]
    fn a_product_is_described_with_its_offer_and_rating() {
        let mut offer = ProductOffer {
            name: "Écran 24\" <Pro>",
            sku: "AOC-24",
            brand: "AOC",
            description: "",
            image: Some("/media/demo/aoc-24.svg"),
            price: Some((11_995, "EUR")),
            in_stock: true,
            rating: Some((4.5, 2)),
        };
        let json = product_json_ld(&offer, "/p/abc");
        assert!(
            json.starts_with(r#"{"@context":"https://schema.org","@type":"Product","name":"#),
            "{json}"
        );
        assert!(!json.contains('<'), "nothing closes the script: {json}");
        assert!(!json.contains("\"description\""), "{json}");
        assert!(
            json.contains(r#""image":"http://127.0.0.1:3000/media/demo/aoc-24.svg""#),
            "{json}"
        );
        assert!(json.contains(r#""price":"119.95","priceCurrency":"EUR","availability":"https://schema.org/InStock""#), "{json}");
        assert!(
            json.contains(r#""url":"http://127.0.0.1:3000/p/abc""#),
            "{json}"
        );
        assert!(
            json.contains(r#""ratingValue":"4.5","reviewCount":2"#),
            "{json}"
        );

        // Out of stock, unrated, unpriced: said, or left out.
        offer.in_stock = false;
        offer.rating = Some((0.0, 0));
        let json = product_json_ld(&offer, "/p/abc");
        assert!(
            json.contains("OutOfStock") && !json.contains("aggregateRating"),
            "{json}"
        );
        offer.price = None;
        assert!(!product_json_ld(&offer, "/p/abc").contains("offers"));
        assert_eq!(json_ld_graph(&["{}".into(), "{}".into()]), "[{},{}]");
    }
}
