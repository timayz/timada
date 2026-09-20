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
    use super::json_string;

    #[test]
    fn nothing_escapes_the_script_element() {
        assert_eq!(
            json_string("</script><b>\"A&B\"\\\n"),
            r#""\u003c/script\u003e\u003cb\u003e\"A\u0026B\"\\\u000a""#
        );
        assert_eq!(json_string("Écrans PC"), "\"Écrans PC\"");
    }
}
