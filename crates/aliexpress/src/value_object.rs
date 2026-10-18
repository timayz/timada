/// The stream of an AliExpress item in a shop.
pub fn aliexpress_product_id(ae_product_id: &str) -> String {
    evento::hash_ids(vec!["aliexpress-product", ae_product_id])
}

/// The stream that says which node renews the token lapsing at
/// `expires_at` (unix seconds).
pub fn token_refresh_id(expires_at: u64) -> String {
    evento::hash_ids(vec!["aliexpress-token-refresh", &expires_at.to_string()])
}

/// How long before an access token lapses it is renewed.
pub const REFRESH_MARGIN_SECS: u64 = 24 * 60 * 60;

/// Who writes what the dashboard does on its own (imports, syncs).
pub const ALIEXPRESS_ACTOR: &str = "timada-aliexpress";

/// How long an import may run before the merchant may ask again: a node
/// that took it on and stopped leaves it there.
pub const IMPORT_STALE_SECS: u64 = 10 * 60;

/// The AliExpress product number in what the merchant typed: the number
/// itself, or an item address (`https://www.aliexpress.com/item/1005006.html`).
pub fn parse_product_id(input: &str) -> Option<String> {
    let input = input.trim();
    let candidate = if input.bytes().all(|b| b.is_ascii_digit()) {
        input
    } else {
        let path = input.split(['?', '#']).next().unwrap_or_default();
        let after = path.rsplit_once("/item/")?.1;
        after.trim_end_matches('/').trim_end_matches(".html")
    };
    ((6..=20).contains(&candidate.len()) && candidate.bytes().all(|b| b.is_ascii_digit()))
        .then(|| candidate.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_product_number_is_read_from_a_number_or_an_address() {
        assert_eq!(
            parse_product_id(" 1005006123456789 ").as_deref(),
            Some("1005006123456789")
        );
        assert_eq!(
            parse_product_id("https://fr.aliexpress.com/item/1005006123456789.html?spm=a2g0o")
                .as_deref(),
            Some("1005006123456789")
        );
        assert_eq!(parse_product_id("https://example.com/p/12"), None);
        assert_eq!(parse_product_id("12"), None);
        assert_eq!(parse_product_id("abc"), None);
    }
}
