use std::fmt;

pub const MIN_RATING: u8 = 1;
pub const MAX_RATING: u8 = 5;
pub const MAX_TITLE_LEN: usize = 100;
pub const MIN_BODY_LEN: usize = 10;
pub const MAX_BODY_LEN: usize = 2000;
pub const MAX_PHOTOS: usize = 3;
pub const MAX_REASON_LEN: usize = 300;
pub const MAX_REPLY_LEN: usize = 1000;

/// How many days after fulfilment a shop mails its buyers, until the
/// merchant says otherwise.
pub const DEFAULT_REQUEST_DELAY_DAYS: u8 = 7;
/// The longest a merchant may wait before asking.
pub const MAX_REQUEST_DELAY_DAYS: u8 = 60;
/// How long a mailed review link opens its order, in seconds.
pub const REVIEW_LINK_TTL_SECS: u64 = 60 * 24 * 60 * 60;

/// A photo of a review, as the upload that took it found it. Frozen in
/// `events.lock`: the fields never change. The file is
/// `<tenant>/<id>.<ext>` wherever the shop keeps its pictures.
#[derive(Debug, Clone, Default, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct ReviewPhoto {
    pub id: String,
    /// Lowercase, no dot: `jpg`, `png`, `webp`, `gif`.
    pub ext: String,
    pub mime: String,
    /// In bytes.
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl ReviewPhoto {
    /// The file name, under the tenant's picture directory.
    pub fn file(&self) -> String {
        format!("{}.{}", self.id, self.ext)
    }
}

/// Where a review stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReviewStatus {
    /// Waiting for the merchant.
    #[default]
    Pending,
    Published,
    Rejected,
}

impl ReviewStatus {
    pub const ALL: [Self; 3] = [Self::Pending, Self::Published, Self::Rejected];

    /// The value stored in the read models.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Published => "published",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "published" => Some(Self::Published),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

impl fmt::Display for ReviewStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The stream of the review of `product_id` from `order_id`: one per product
/// and order.
pub fn review_id(order_id: &str, product_id: &str) -> String {
    evento::hash_ids(vec!["review", order_id, product_id])
}

/// Aggregate id of the shop's review-request policy: one per tenant store.
pub fn review_requests_id() -> String {
    evento::hash_ids(vec!["review-requests"])
}

/// Aggregate id of the review request mailed for an order.
pub fn review_request_id(order_id: &str) -> String {
    evento::hash_ids(vec!["review-request", order_id])
}

/// Aggregate id of the link a token opens. Anyone reading the store sees
/// only this, never the token.
pub fn review_link_id(token: &str) -> String {
    evento::hash_ids(vec!["review-link", token])
}

/// A fresh link token: 32 random bytes as URL-safe base64 without padding
/// (43 characters).
pub(crate) fn generate_token() -> String {
    use base64::Engine;
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// How a review is signed, from the name an order was shipped to: the first
/// word and the initial of the last one (`Ada Lovelace` → `Ada L.`), the
/// single word alone, nothing for a blank name.
pub fn author_name(shipping_name: &str) -> String {
    let mut words = shipping_name.split_whitespace();
    let Some(first) = words.next() else {
        return String::new();
    };
    match words.last().and_then(|last| last.chars().next()) {
        Some(initial) => format!("{first} {}.", initial.to_uppercase()),
        None => first.to_owned(),
    }
}

/// A rating as stars: `★★★★☆` for 4.
pub fn stars(rating: u8) -> String {
    let full = rating.min(MAX_RATING) as usize;
    format!(
        "{}{}",
        "★".repeat(full),
        "☆".repeat(MAX_RATING as usize - full)
    )
}

/// Trims a text and turns blank into `None`.
pub(crate) fn non_blank(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_is_signed_with_a_first_name_and_an_initial() {
        assert_eq!(author_name("Ada Lovelace"), "Ada L.");
        assert_eq!(author_name("  ada  king  lovelace "), "ada L.");
        assert_eq!(author_name("Ada"), "Ada");
        assert_eq!(author_name("Émile zola"), "Émile Z.");
        assert_eq!(author_name("  "), "");
    }

    #[test]
    fn statuses_read_back() {
        for status in ReviewStatus::ALL {
            assert_eq!(ReviewStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(ReviewStatus::parse("other"), None);
    }

    #[test]
    fn stars_are_five() {
        assert_eq!(stars(4), "★★★★☆");
        assert_eq!(stars(9), "★★★★★");
    }
}
