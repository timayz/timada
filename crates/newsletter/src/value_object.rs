use std::fmt;

use base64::Engine;
use hmac::{Hmac, Mac};
use timada_core::Key;

use crate::error::NewsletterError;

pub use timada_core::MAX_EMAIL_LEN;

/// How long the link mailed to confirm a subscription works, in seconds.
pub const CONFIRM_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// How long after a confirmation link is mailed another may be asked for the
/// same address, in seconds: asking again sooner mails nothing, so a form
/// cannot be used to flood someone's inbox.
pub const RESEND_AFTER_SECS: u64 = 60 * 60;

/// Asked from the storefront's sign-up form.
pub const SOURCE_FORM: &str = "form";
/// Ticked at checkout.
pub const SOURCE_CHECKOUT: &str = "checkout";
/// Asked by a shopper signed in to their account.
pub const SOURCE_ACCOUNT: &str = "account";
/// Every place a subscription may be asked from, as stored.
pub const SOURCES: [&str; 3] = [SOURCE_FORM, SOURCE_CHECKOUT, SOURCE_ACCOUNT];

/// The longest subject of a campaign, in characters.
pub const MAX_CAMPAIGN_SUBJECT_LEN: usize = 200;
/// The longest body of a campaign (HTML or plain text), in bytes.
pub const MAX_CAMPAIGN_BODY_LEN: usize = 200 * 1024;

/// Where a campaign stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CampaignStatus {
    /// Being written: not sent to anyone.
    #[default]
    Draft,
    /// Going out, a batch at a time.
    Sending,
    Sent,
    /// Going out on its schedule, an edition per cycle.
    Scheduled,
    /// Its schedule was stopped by the merchant.
    Stopped,
}

impl CampaignStatus {
    pub const ALL: [Self; 5] = [
        Self::Draft,
        Self::Sending,
        Self::Sent,
        Self::Scheduled,
        Self::Stopped,
    ];

    /// The value stored in the read model.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Scheduled => "scheduled",
            Self::Stopped => "stopped",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }
}

/// A fresh id for a campaign: a ULID, sortable by when it was made.
pub fn new_campaign_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The id of one cycle of a scheduled campaign
/// ([`Edition`](crate::aggregator::Edition)), by its first cycle.
pub fn edition_id(campaign_id: &str, first_cycle: &str) -> String {
    evento::hash_ids(vec!["newsletter-edition", campaign_id, first_cycle])
}

/// The id of one campaign's (or edition's) mail to one subscriber
/// ([`Delivery`](crate::aggregator::Delivery)).
pub fn delivery_id(campaign_id: &str, subscriber_id: &str) -> String {
    evento::hash_ids(vec!["newsletter-delivery", campaign_id, subscriber_id])
}

/// Where a subscriber stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SubscriberStatus {
    /// Asked, the link not followed yet: not mailed any news.
    #[default]
    Pending,
    Confirmed,
    Unsubscribed,
}

impl SubscriberStatus {
    pub const ALL: [Self; 3] = [Self::Pending, Self::Confirmed, Self::Unsubscribed];

    /// The value stored in the read model.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Unsubscribed => "unsubscribed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }
}

impl fmt::Display for SubscriberStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Aggregate id of the subscriber behind a normalized address. It is a plain
/// hash: anyone holding a candidate address can test it, even once the
/// subscriber is erased (as with the shoppers' ids).
pub fn subscriber_id(email: &str) -> String {
    evento::hash_ids(vec!["newsletter-subscriber", email])
}

/// The canonical form of an address: trimmed, lower-cased, one `@` with
/// something on both sides, nothing that is not printable. (The orders,
/// shoppers and support contexts have the same rule; the crates do not
/// depend on each other.)
pub fn normalize_email(email: &str) -> Result<String, NewsletterError> {
    timada_core::normalize_email(email).ok_or(NewsletterError::InvalidEmail)
}

/// One of [`SOURCES`].
pub fn check_source(source: &str) -> Result<&'static str, NewsletterError> {
    SOURCES
        .into_iter()
        .find(|known| *known == source)
        .ok_or(NewsletterError::InvalidSource)
}

/// The MAC of `parts` under the subscriber's key, each part length-prefixed
/// so no two lists of parts sign alike.
fn mac(key: &Key, parts: &[&str]) -> Hmac<sha2::Sha256> {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(b"timada-newsletter/link");
    for part in parts {
        mac.update(&(part.len() as u32).to_be_bytes());
        mac.update(part.as_bytes());
    }
    mac
}

fn encode(mac: Hmac<sha2::Sha256>) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn verify(mac: Hmac<sha2::Sha256>, tag: &str) -> bool {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(tag)
        .is_ok_and(|tag| mac.verify_slice(&tag).is_ok())
}

/// The link that confirms one request: `{subscriber}.{requested_at}.{mac}`.
/// It names the request by when it was written, so it confirms that one
/// only: a later request (or leaving the list) makes it stale.
pub fn confirm_token(key: &Key, subscriber_id: &str, key_id: &str, requested_at: u64) -> String {
    let requested_at = requested_at.to_string();
    let tag = encode(mac(key, &["confirm", subscriber_id, key_id, &requested_at]));
    format!("{subscriber_id}.{requested_at}.{tag}")
}

/// The link that takes the subscriber off the list: `{subscriber}.{mac}`. It
/// never expires, since every mail of news carries it; erasing the
/// subscriber destroys the key that signs it.
pub fn unsubscribe_token(key: &Key, subscriber_id: &str, key_id: &str) -> String {
    let tag = encode(mac(key, &["unsubscribe", subscriber_id, key_id]));
    format!("{subscriber_id}.{tag}")
}

/// A confirmation link, read but not checked yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmLink<'a> {
    pub subscriber_id: &'a str,
    pub requested_at: u64,
    tag: &'a str,
}

impl<'a> ConfirmLink<'a> {
    pub fn parse(token: &'a str) -> Option<Self> {
        let mut parts = token.trim().splitn(3, '.');
        let subscriber_id = parts.next().filter(|id| !id.is_empty())?;
        let requested_at = parts.next()?.parse().ok()?;
        let tag = parts.next()?;
        Some(Self {
            subscriber_id,
            requested_at,
            tag,
        })
    }

    pub fn verify(&self, key: &Key, key_id: &str) -> bool {
        let requested_at = self.requested_at.to_string();
        verify(
            mac(key, &["confirm", self.subscriber_id, key_id, &requested_at]),
            self.tag,
        )
    }
}

/// An unsubscribe link, read but not checked yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsubscribeLink<'a> {
    pub subscriber_id: &'a str,
    tag: &'a str,
}

impl<'a> UnsubscribeLink<'a> {
    pub fn parse(token: &'a str) -> Option<Self> {
        let (subscriber_id, tag) = token.trim().split_once('.')?;
        (!subscriber_id.is_empty() && !tag.contains('.')).then_some(Self { subscriber_id, tag })
    }

    pub fn verify(&self, key: &Key, key_id: &str) -> bool {
        verify(
            mac(key, &["unsubscribe", self.subscriber_id, key_id]),
            self.tag,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_read_back() {
        for status in SubscriberStatus::ALL {
            assert_eq!(SubscriberStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(SubscriberStatus::parse("other"), None);
        for status in CampaignStatus::ALL {
            assert_eq!(CampaignStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(CampaignStatus::parse("other"), None);
    }

    #[test]
    fn a_delivery_names_one_pair() {
        assert_eq!(delivery_id("c1", "s1"), delivery_id("c1", "s1"));
        assert_ne!(delivery_id("c1", "s1"), delivery_id("c1", "s2"));
        assert_ne!(delivery_id("c1", "s1"), delivery_id("c2", "s1"));
    }

    #[test]
    fn an_address_is_normalised_or_refused() {
        assert_eq!(
            normalize_email("  Ada@Example.com ").unwrap(),
            "ada@example.com"
        );
        for bad in [
            "",
            "ada",
            "@example.com",
            "ada@",
            "a@b@c",
            "ada @example.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn sources_are_known_ones() {
        assert_eq!(check_source("checkout").unwrap(), SOURCE_CHECKOUT);
        assert!(check_source("elsewhere").is_err());
    }

    #[test]
    fn a_confirmation_link_checks_out_under_its_key_only() {
        let key = Key::generate();
        let token = confirm_token(&key, "sub", "k1", 1_700_000_000);
        let link = ConfirmLink::parse(&token).unwrap();
        assert_eq!(link.subscriber_id, "sub");
        assert_eq!(link.requested_at, 1_700_000_000);
        assert!(link.verify(&key, "k1"));
        assert!(!link.verify(&key, "k2"));
        assert!(!link.verify(&Key::generate(), "k1"));
        // Another request time is another link.
        let moved = token.replace("1700000000", "1700000001");
        assert!(!ConfirmLink::parse(&moved).unwrap().verify(&key, "k1"));
        for bad in ["", "sub", "sub.x.tag", ".1.tag"] {
            assert!(ConfirmLink::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn an_unsubscribe_link_is_not_a_confirmation_link() {
        let key = Key::generate();
        let token = unsubscribe_token(&key, "sub", "k1");
        let link = UnsubscribeLink::parse(&token).unwrap();
        assert!(link.verify(&key, "k1"));
        assert!(!link.verify(&key, "k2"));
        assert!(ConfirmLink::parse(&token).is_none());
        let confirm = confirm_token(&key, "sub", "k1", 1);
        assert!(UnsubscribeLink::parse(&confirm).is_none());
    }
}
