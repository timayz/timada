//! What every bounded context shares: the refusal code of a write the
//! store is not taking right now, the reading of a shopper's language and
//! time zone, the keys personal data is sealed under ([`keys`]), what a
//! stream keeps of a mailed code ([`codes`]), and the small rules every
//! context would otherwise write for itself: the canonical form of an
//! e-mail address ([`email`]), of a slug ([`slug`]) and of a currency
//! ([`currency`]), the clock ([`time`]), calendar dates ([`civil`]), an
//! amount for a person to read ([`money`]) and a `LIKE` pattern ([`sql`]),
//! and the database the read models share with the store ([`db`]). No web
//! framework: contexts depend on this crate and on evento, nothing more
//! between them.

pub mod civil;
pub mod codes;
pub mod currency;
pub mod db;
pub mod email;
pub mod keys;
pub mod money;
pub mod slug;
pub mod sql;
pub mod time;

use evento::WriteError;

pub use codes::{CodeVerifier, HmacCodeVerifier};
pub use currency::{CURRENCIES, normalize_currency};
pub use email::{MAX_EMAIL_LEN, normalize_email};
pub use keys::{Key, Keys};
pub use money::{currency_symbol, format_amount, format_money, format_money_range};
pub use slug::{
    MIN_SLUG_LEN, SLUG_SUFFIX_LEN, normalize_slug, slugify, suffixed_slug, truncate_slug,
};
pub use sql::like_contains;
pub use time::unix_now;

/// The refusal code pages and the store API carry when a tenant is not
/// taking writes for the moment: a node of the cluster is away and the
/// tenant keeps its writes for it up to the backlog, or this node is being
/// copied back from a peer. Nothing about the request is wrong; it goes
/// through again once the node is back or given up.
pub const WRITES_PAUSED: &str = "writes_paused";

/// `Some(WRITES_PAUSED)` when `err` is a write the store refused for the
/// moment ([`WriteError::Unavailable`]), `None` for any other write failure.
pub fn write_refusal(err: &WriteError) -> Option<&'static str> {
    match err {
        WriteError::Unavailable(_) => Some(WRITES_PAUSED),
        _ => None,
    }
}

/// The primary subtag of a language tag or of an `Accept-Language` value,
/// lowercased: `"fr-FR,fr;q=0.9,en;q=0.8"` and `"EN_us"` give `"fr"` and
/// `"en"`. Only the first entry counts, and only when it is two or three
/// ASCII letters (ISO 639): `"*"`, a blank or a private tag give `None`.
pub fn primary_language(value: &str) -> Option<String> {
    let first = value.split(',').next()?;
    let tag = first.split(';').next()?.trim();
    let primary = tag.split(['-', '_']).next()?;
    let ok = (2..=3).contains(&primary.len()) && primary.bytes().all(|b| b.is_ascii_alphabetic());
    ok.then(|| primary.to_ascii_lowercase())
}

/// The longest time zone name kept, in bytes (the longest IANA name is 32).
pub const MAX_TIME_ZONE_LEN: usize = 64;

/// A time zone name from the IANA database, as the database spells it:
/// `" europe/paris "` gives `"Europe/Paris"`. Anything the database does not
/// name (a blank, an offset, garbage) gives `None`.
pub fn time_zone(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_TIME_ZONE_LEN {
        return None;
    }
    let zone = jiff::tz::TimeZone::get(value).ok()?;
    let name = zone.iana_name()?;
    // Fixed offsets and POSIX rules have no IANA name.
    Some(name.to_owned())
}

/// Every time zone name the database knows, sorted, for a form to offer.
pub fn time_zones() -> Vec<String> {
    let mut names: Vec<String> = jiff::tz::db()
        .available()
        .map(|name| name.to_string())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primary_language_is_the_first_tags_first_subtag() {
        assert_eq!(primary_language("fr").as_deref(), Some("fr"));
        assert_eq!(
            primary_language("fr-FR,fr;q=0.9,en;q=0.8").as_deref(),
            Some("fr")
        );
        assert_eq!(primary_language(" EN-us ").as_deref(), Some("en"));
        assert_eq!(primary_language("EN_us").as_deref(), Some("en"));
        assert_eq!(primary_language("zh-Hant-TW").as_deref(), Some("zh"));
        assert_eq!(primary_language("ast").as_deref(), Some("ast"));
        assert_eq!(primary_language("de;q=0.5").as_deref(), Some("de"));
    }

    #[test]
    fn anything_but_a_language_reads_as_none() {
        for value in [
            "",
            "   ",
            "*",
            "*;q=0.1",
            "i-klingon",
            "abcd",
            "f",
            "1234",
            "fr1",
        ] {
            assert_eq!(primary_language(value), None, "{value:?}");
        }
    }

    #[test]
    fn a_time_zone_is_spelled_as_the_database_does() {
        assert_eq!(time_zone("Europe/Paris").as_deref(), Some("Europe/Paris"));
        assert_eq!(time_zone(" europe/paris ").as_deref(), Some("Europe/Paris"));
        assert_eq!(time_zone("UTC").as_deref(), Some("UTC"));
        for value in ["", "  ", "Mars/Olympus", "+02:00", "../../etc/passwd"] {
            assert_eq!(time_zone(value), None, "{value:?}");
        }
        assert!(time_zones().iter().any(|name| name == "America/New_York"));
    }

    #[test]
    fn only_a_paused_store_is_a_refusal() {
        let paused = WriteError::Unavailable(anyhow::anyhow!("backlog at cap"));
        assert_eq!(write_refusal(&paused), Some(WRITES_PAUSED));
        let broken = WriteError::Unknown(anyhow::anyhow!("disk gone"));
        assert_eq!(write_refusal(&broken), None);
        assert_eq!(write_refusal(&WriteError::InvalidOriginalVersion), None);
    }
}
