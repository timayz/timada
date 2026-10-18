//! What every bounded context shares: the refusal code of a write the
//! store is not taking right now, the reading of a shopper's language, and
//! the keys personal data is sealed under ([`keys`]).

pub mod keys;

use evento::WriteError;

pub use keys::{Key, Keys};

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
    fn only_a_paused_store_is_a_refusal() {
        let paused = WriteError::Unavailable(anyhow::anyhow!("backlog at cap"));
        assert_eq!(write_refusal(&paused), Some(WRITES_PAUSED));
        let broken = WriteError::Unknown(anyhow::anyhow!("disk gone"));
        assert_eq!(write_refusal(&broken), None);
        assert_eq!(write_refusal(&WriteError::InvalidOriginalVersion), None);
    }
}
