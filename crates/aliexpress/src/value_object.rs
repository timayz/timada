/// The stream of an AliExpress item in a shop.
pub fn aliexpress_product_id(ae_product_id: &str) -> String {
    evento::hash_ids(vec!["aliexpress-product", ae_product_id])
}

/// How long a node's claim on a token renewal holds without the tokens
/// AliExpress answered being recorded: past it, the node is taken to have
/// stopped, and another takes the renewal over. Far beyond a call's timeout.
pub const TOKEN_CLAIM_STALE_SECS: u64 = 10 * 60;

/// The tokens a renewal got from AliExpress, sealed for the company, and
/// when they lapse (unix seconds; `refresh_expires_at` 0 for never).
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct RenewedTokens {
    pub access_token_sealed: Vec<u8>,
    pub refresh_token_sealed: Vec<u8>,
    pub expires_at: u64,
    pub refresh_expires_at: u64,
    pub account: String,
}

impl RenewedTokens {
    /// Whether they renew the access lapsing at `expires_at`: theirs lapses
    /// later. AliExpress may answer with the lifetime it gave last time,
    /// counted from the same second, and then nothing is renewed.
    pub fn renews(&self, expires_at: u64) -> bool {
        self.expires_at > expires_at
    }
}

/// The stream that says which node renews the token of `company_id` lapsing
/// at `expires_at` (unix seconds).
pub fn token_refresh_id(company_id: &str, expires_at: u64) -> String {
    evento::hash_ids(vec![
        "aliexpress-token-refresh",
        company_id,
        &expires_at.to_string(),
    ])
}

/// The stream that places a shop's order on AliExpress.
pub fn forward_id(order_id: &str) -> String {
    evento::hash_ids(vec!["aliexpress-forward", order_id])
}

/// The platform's stream of the AliExpress order `ae_order_id`: the forward
/// that placed it, and the pushes for it.
pub fn ae_order_index_id(ae_order_id: &str) -> String {
    evento::hash_ids(vec!["aliexpress-order", ae_order_id])
}

/// The shop's stream saying which AliExpress item filled the catalog product
/// `product_id`.
pub fn product_link_id(product_id: &str) -> String {
    evento::hash_ids(vec!["aliexpress-product-link", product_id])
}

/// How long an order placed on AliExpress is read again for tracking.
pub const FORWARD_WATCH_SECS: u64 = 60 * 24 * 60 * 60;

/// How long after an attempt that bought nothing a forward is placed again
/// on its own, try after try (#470): five tries over about a day, so an
/// outage of AliExpress or of the payment provider asks no merchant to place
/// each order again by hand. Past the last, the merchant decides.
pub const FORWARD_RETRY_DELAYS_SECS: [u64; 5] =
    [15 * 60, 60 * 60, 3 * 60 * 60, 6 * 60 * 60, 12 * 60 * 60];

/// How long a forward may stay taken on with no word of how placing it went:
/// past it, the node placing it stopped, or could not record what AliExpress
/// answered, and AliExpress may have the order. Well above what placing takes,
/// a couple of calls per item each given twenty seconds.
pub const FORWARD_STALL_SECS: u64 = 30 * 60;

/// The most a parcel shipped into the EU from outside may be worth, VAT
/// aside, in euro cents, for its goods to pay the flat customs duty rather
/// than their own ([`ImportCharge`](crate::ImportCharge)).
pub const IMPORT_LOW_VALUE_EURO_CENTS: u64 = 15_000;

/// How long before an access token lapses it is renewed, at most: a quarter
/// of its lifetime when that is shorter ([`refresh_margin`]).
pub const REFRESH_MARGIN_SECS: u64 = 24 * 60 * 60;

/// How long before an access token given `lifetime_secs` to live lapses it
/// is renewed: a quarter of its life, a day at most (#472). A margin as long
/// as a token lasting a day or less would renew it at every look; a quarter
/// renews it about once a lifetime. A lifetime not known (0) takes the day.
pub fn refresh_margin(lifetime_secs: u64) -> u64 {
    if lifetime_secs == 0 {
        REFRESH_MARGIN_SECS
    } else {
        REFRESH_MARGIN_SECS.min(lifetime_secs / 4)
    }
}

/// Who writes what the dashboard does on its own (imports, syncs).
pub const ALIEXPRESS_ACTOR: &str = "timada-aliexpress";
/// The supplier an order names when some of its goods are bought on
/// AliExpress once it is placed (`Order::Placed.supplier`).
pub const ALIEXPRESS_SUPPLIER: &str = "aliexpress";

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
    fn a_token_is_renewed_a_quarter_of_its_life_early_a_day_at_most() {
        assert_eq!(refresh_margin(30 * 86_400), REFRESH_MARGIN_SECS);
        assert_eq!(refresh_margin(86_400), 6 * 3_600);
        assert_eq!(refresh_margin(8 * 3_600), 2 * 3_600);
        assert_eq!(refresh_margin(0), REFRESH_MARGIN_SECS);
    }

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
