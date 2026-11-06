/// The stream of an AliExpress item in a shop.
pub fn aliexpress_product_id(ae_product_id: &str) -> String {
    evento::hash_ids(vec!["aliexpress-product", ae_product_id])
}

/// The stream that says which node renews the token of `company_id` lapsing
/// at `expires_at` (unix seconds).
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

/// How long an order placed on AliExpress is read again for tracking.
pub const FORWARD_WATCH_SECS: u64 = 60 * 24 * 60 * 60;

/// How long before an access token lapses it is renewed.
pub const REFRESH_MARGIN_SECS: u64 = 24 * 60 * 60;

/// Who writes what the dashboard does on its own (imports, syncs).
pub const ALIEXPRESS_ACTOR: &str = "timada-aliexpress";
/// The supplier an order names when some of its goods are bought on
/// AliExpress once it is placed (`Order::Placed.supplier`).
pub const ALIEXPRESS_SUPPLIER: &str = "aliexpress";

/// How long an import may run before the merchant may ask again: a node
/// that took it on and stopped leaves it there.
pub const IMPORT_STALE_SECS: u64 = 10 * 60;

/// Which of the nodes up at the moment this one is. Every linked item, and every
/// order placed on AliExpress, belongs to one of them, so one node reads it and
/// the app's quota is spent once. Two nodes that disagree for a moment on who is
/// up read one twice or leave it for later, neither of which harms it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Share {
    /// This node's place among them, from 0.
    pub index: u64,
    /// How many there are, at least 1.
    pub of: u64,
}

impl Share {
    /// A node on its own: everything is its.
    pub const ALONE: Self = Self { index: 0, of: 1 };

    /// Whether the item or order `id` names belongs to this node.
    pub fn owns(&self, id: &str) -> bool {
        u64::from(shard_of(id)) % self.of == self.index
    }
}

/// Where an item or order falls among the nodes (FNV-1a of its id): the same on
/// every node and in every build, which the standard hasher does not promise.
pub fn shard_of(id: &str) -> u32 {
    id.bytes().fold(0x811c_9dc5, |hash: u32, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

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
    fn every_item_belongs_to_one_node() {
        let ids: Vec<String> = (0..300).map(|n| format!("link-{n}")).collect();
        for of in 1..=5 {
            let mut held = 0;
            for index in 0..of {
                let share = Share { index, of };
                let own = ids.iter().filter(|id| share.owns(id)).count();
                // Each node holds its part, near enough.
                assert!(own >= 300 / of as usize / 2, "{own} of 300 on {index}/{of}");
                held += own;
            }
            assert_eq!(held, ids.len());
        }
    }

    #[test]
    fn a_node_alone_owns_everything() {
        assert!(Share::ALONE.owns("anything"));
        assert!(Share::ALONE.owns(""));
    }

    #[test]
    fn where_an_item_falls_does_not_depend_on_the_build() {
        assert_eq!(shard_of(""), 0x811c_9dc5);
        assert_eq!(shard_of("a"), 0xe40c_292c);
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
