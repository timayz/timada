//! Where the shopper comes back to after an action: the page the cart drawer
//! sat on, the checkout, the cart; and the guards that keep a form from
//! sending anyone off the site.

use topcoat_core::context::Cx;

/// Where a cart form sends the shopper back: `checkout` when it was sent
/// from the checkout's summary, `drawer` from the cart drawer on some page
/// (`from`), the cart page otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Back {
    #[default]
    Cart,
    Checkout,
    Drawer,
}

impl Back {
    /// The value a hidden `back` field carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cart => "",
            Self::Checkout => "checkout",
            Self::Drawer => "drawer",
        }
    }

    /// What a form said; anything unknown is the cart.
    pub fn parse(value: &str) -> Self {
        match value {
            "checkout" => Self::Checkout,
            "drawer" => Self::Drawer,
            _ => Self::Cart,
        }
    }
}

/// Where a cart form sends the shopper back, with the refusal to show there
/// by code. `from` is a path on this site, so a form cannot send anyone
/// elsewhere.
pub fn back_url(cx: &Cx, back: Back, from: &str, error: Option<&str>) -> String {
    let paths = crate::paths(cx);
    match back {
        Back::Checkout => paths.checkout_url(error),
        Back::Drawer => drawer_url(from, error),
        Back::Cart => paths.cart_url(error),
    }
}

/// `from` with the drawer open: `#cart`, and the refusal to show in it as
/// `cart_error`. Anything but a path on this site (`//host`, `/\host`, a
/// full URL) is the home page; refusals it already carried are dropped.
pub fn drawer_url(from: &str, error: Option<&str>) -> String {
    let from = from.split('#').next().unwrap_or_default();
    let from = if from.starts_with('/') && !from.starts_with("//") && !from.starts_with("/\\") {
        from
    } else {
        "/"
    };
    let (path, query) = from.split_once('?').unwrap_or((from, ""));
    let mut pairs: Vec<String> = query
        .split('&')
        .filter(|pair| {
            !pair.is_empty() && !pair.starts_with("cart_error=") && !pair.starts_with("error=")
        })
        .map(str::to_owned)
        .collect();
    if let Some(code) = error {
        // A code is snake_case: nothing to escape.
        pairs.push(format!("cart_error={code}"));
    }
    if pairs.is_empty() {
        format!("{path}#cart")
    } else {
        format!("{path}?{}#cart", pairs.join("&"))
    }
}

/// Where to go after signing in: `next` when it is a path on this site,
/// else `fallback`.
pub fn safe_next(next: Option<&str>, fallback: String) -> String {
    match next {
        Some(next) if next.starts_with('/') && !next.starts_with("//") && !next.contains('\\') => {
            next.to_owned()
        }
        _ => fallback,
    }
}

/// The page the request is for, as the shopper sees it in the address bar
/// (path and query), for a form to come back to.
pub fn current_path(cx: &Cx) -> String {
    topcoat_router::request::original_uri(cx)
        .path_and_query()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| "/".to_owned())
}

/// `value` as a query-string value: everything but the unreserved
/// characters percent-encoded.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `?a=1&b=2` from the pairs with a value; empty when none has one.
pub fn query(pairs: &[(&str, Option<&str>)]) -> String {
    let joined: Vec<String> = pairs
        .iter()
        .filter_map(|(key, value)| value.map(|value| format!("{key}={}", encode(value))))
        .collect();
    if joined.is_empty() {
        String::new()
    } else {
        format!("?{}", joined.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawer_url_keeps_the_page_and_opens_the_drawer() {
        assert_eq!(drawer_url("/p/mug", None), "/p/mug#cart");
        assert_eq!(drawer_url("/p/mug#reviews", None), "/p/mug#cart");
        assert_eq!(
            drawer_url("/c/toys?sort=price_asc&page=2", Some("invalid_quantity")),
            "/c/toys?sort=price_asc&page=2&cart_error=invalid_quantity#cart"
        );
        assert_eq!(
            drawer_url("/p/mug?cart_error=old&error=older", None),
            "/p/mug#cart"
        );
    }

    #[test]
    fn drawer_url_refuses_other_sites() {
        assert_eq!(drawer_url("//evil.example/x", None), "/#cart");
        assert_eq!(drawer_url("/\\evil.example", None), "/#cart");
        assert_eq!(drawer_url("https://evil.example/", None), "/#cart");
        assert_eq!(drawer_url("", None), "/#cart");
    }

    #[test]
    fn safe_next_only_takes_a_path_here() {
        assert_eq!(safe_next(Some("/account"), "/".into()), "/account");
        assert_eq!(safe_next(Some("//x"), "/".into()), "/");
        assert_eq!(safe_next(Some("/a\\b"), "/".into()), "/");
        assert_eq!(safe_next(None, "/".into()), "/");
    }

    #[test]
    fn query_encodes_values() {
        assert_eq!(
            query(&[("q", Some("chat noir")), ("page", Some("2"))]),
            "?q=chat%20noir&page=2"
        );
        assert_eq!(query(&[("error", None)]), "");
        assert_eq!(encode("é/&"), "%C3%A9%2F%26");
    }
}
