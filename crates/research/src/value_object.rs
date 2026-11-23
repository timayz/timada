//! The small rules of a study: what a name, a seed, a country and a
//! language look like, and the bounds a fetch keeps to.

use ulid::Ulid;

use crate::error::ResearchError;

/// The longest name of a study, in characters.
pub const MAX_NAME_LEN: usize = 80;

/// How many seeds a study holds at most: each is a paid call to fetch.
pub const MAX_SEEDS: usize = 20;

/// The longest seed, in characters.
pub const MAX_SEED_LEN: usize = 80;

/// How many keywords one fetch records at most. The rows live in the event
/// that records the fetch, so the cap bounds what one event weighs.
pub const MAX_FETCH_ROWS: usize = 1000;

/// The longest keyword a fetch records, in characters; a longer one is cut.
pub const MAX_KEYWORD_LEN: usize = 200;

/// The longest reason a failed fetch keeps, in characters.
pub const MAX_REASON_LEN: usize = 500;

/// How many brands, and how many excluded words, a study lists at most.
pub const MAX_LISTED_WORDS: usize = 200;

/// The longest brand or excluded word, in characters.
pub const MAX_WORD_LEN: usize = 80;

/// How many keywords a study's owner drops by hand at most, and brings
/// back at most.
pub const MAX_HAND_SET: usize = 2_000;

/// How many events a study takes before it refuses what its owner changes:
/// a stream holds 65,535, and the fetches still on their way need room.
pub const MAX_STUDY_EVENTS: u16 = 60_000;

/// How many of a study's main keywords, the most searched, have their first
/// results looked at: each is a paid search.
pub const MAX_CHECKED_KEYWORDS: usize = 50;

/// How many of a search's first organic results are kept: its first page.
pub const MAX_RESULTS: usize = 10;

/// The longest site name, in characters.
pub const MAX_DOMAIN_LEN: usize = 253;

/// The longest address of a result kept, in characters; a longer one is
/// cut.
pub const MAX_URL_LEN: usize = 500;

/// The longest title of a result kept, in characters; a longer one is cut.
pub const MAX_TITLE_LEN: usize = 200;

/// How many sites one user marks at most.
pub const MAX_MARKS: usize = 2_000;

/// How many times a user's marks change before they are refused: a stream
/// holds 65,535 events.
pub const MAX_MARK_EVENTS: u16 = 60_000;

/// How long a fetch may run before the owner may ask again: a node that
/// took it on and stopped leaves it there.
pub const FETCH_STALE_SECS: u64 = 10 * 60;

/// How many calls a day may hold room for at most, whatever the caps say:
/// each writes two events to the day's stream, which is read whole before
/// every call and holds 65,535 events at most.
pub const MAX_DAY_HOLDS: u32 = 20_000;

/// Who writes what the host does on its own (the fetches).
pub const RESEARCH_ACTOR: &str = "timada-research";

/// What a searcher may be after, as a row's `intent` says it.
pub const INTENTS: [&str; 4] = [
    "informational",
    "navigational",
    "commercial",
    "transactional",
];

/// A fresh study id.
pub fn new_study_id() -> String {
    Ulid::generate().to_string()
}

/// `text` with its runs of whitespace as one space each, trimmed.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A study's name: trimmed, 1 to [`MAX_NAME_LEN`] characters.
pub fn normalize_name(name: &str) -> Result<String, ResearchError> {
    let name = collapse(name);
    let len = name.chars().count();
    if len == 0 || len > MAX_NAME_LEN || name.chars().any(char::is_control) {
        return Err(ResearchError::InvalidName);
    }
    Ok(name)
}

/// A seed keyword: lowercased, its spaces single, 1 to [`MAX_SEED_LEN`]
/// characters. A search engine reads `Arbre  à Chat` and `arbre à chat` as
/// one query, so a study holds it once.
pub fn normalize_seed(seed: &str) -> Result<String, ResearchError> {
    let seed = collapse(seed).to_lowercase();
    let len = seed.chars().count();
    if len == 0 || len > MAX_SEED_LEN || seed.chars().any(char::is_control) {
        return Err(ResearchError::InvalidSeed);
    }
    Ok(seed)
}

/// The seeds of a new study, each normalized, in the order given, a seed
/// given twice kept once: one at least, [`MAX_SEEDS`] at most.
pub fn normalize_seeds<S: AsRef<str>>(seeds: &[S]) -> Result<Vec<String>, ResearchError> {
    let mut kept: Vec<String> = Vec::new();
    for seed in seeds {
        if seed.as_ref().trim().is_empty() {
            continue;
        }
        let seed = normalize_seed(seed.as_ref())?;
        if !kept.contains(&seed) {
            kept.push(seed);
        }
    }
    if kept.is_empty() {
        return Err(ResearchError::NoSeed);
    }
    if kept.len() > MAX_SEEDS {
        return Err(ResearchError::TooManySeeds);
    }
    Ok(kept)
}

/// A list of brands or of excluded words as the owner typed it: each
/// trimmed, its spaces single, a blank one passed over, one given twice
/// kept once. [`MAX_LISTED_WORDS`] at most, each of [`MAX_WORD_LEN`]
/// characters at most.
pub fn normalize_words<S: AsRef<str>>(words: &[S]) -> Result<Vec<String>, ResearchError> {
    let mut kept: Vec<String> = Vec::new();
    for word in words {
        let word = collapse(word.as_ref());
        if word.is_empty() {
            continue;
        }
        if word.chars().count() > MAX_WORD_LEN || word.chars().any(char::is_control) {
            return Err(ResearchError::InvalidWord);
        }
        if !kept.contains(&word) {
            kept.push(word);
        }
    }
    if kept.len() > MAX_LISTED_WORDS {
        return Err(ResearchError::TooManyWords);
    }
    Ok(kept)
}

/// A keyword as a row spells it: on one line, [`MAX_KEYWORD_LEN`]
/// characters at most.
pub fn normalize_keyword(keyword: &str) -> Result<String, ResearchError> {
    let keyword = collapse(keyword);
    if keyword.is_empty() || keyword.chars().count() > MAX_KEYWORD_LEN {
        return Err(ResearchError::InvalidKeyword);
    }
    Ok(keyword)
}

/// A site as its marks and its results name it: the host, lowercase,
/// without `www.` in front nor a dot behind (`WWW.Zooplus.fr` gives
/// `zooplus.fr`). A subdomain is a site of its own: no list of public
/// suffixes says where a name ends.
pub fn normalize_domain(domain: &str) -> Result<String, ResearchError> {
    let domain = domain.trim().trim_end_matches('.').to_lowercase();
    let domain = domain.strip_prefix("www.").unwrap_or(&domain);
    let label = |label: &str| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_alphanumeric() || c == '-')
    };
    if domain.chars().count() > MAX_DOMAIN_LEN
        || !domain.contains('.')
        || !domain.split('.').all(label)
    {
        return Err(ResearchError::InvalidDomain);
    }
    Ok(domain.to_owned())
}

/// A country as ISO 3166-1 alpha-2, uppercase: ` fr ` gives `FR`.
pub fn normalize_country(country: &str) -> Result<String, ResearchError> {
    let country = country.trim();
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(ResearchError::InvalidPlace);
    }
    Ok(country.to_ascii_uppercase())
}

/// A language code, lowercase: `FR` gives `fr`, `zh-TW` gives `zh-tw`. Two
/// or three letters, then subtags of letters and digits; which languages a
/// country is searched in is the keyword source's to say, not this crate's.
pub fn normalize_language(language: &str) -> Result<String, ResearchError> {
    let language = language.trim().to_ascii_lowercase();
    let mut subtags = language.split('-');
    let primary = subtags.next().unwrap_or_default();
    let ok = (2..=3).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_lowercase())
        && subtags.all(|subtag| {
            (1..=8).contains(&subtag.len()) && subtag.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        && language.len() <= 16;
    if !ok {
        return Err(ResearchError::InvalidPlace);
    }
    Ok(language)
}

/// The day (UTC) `now` (unix seconds) falls on, as `2026-11-23`: the id of
/// the stream that counts what the day's calls cost.
pub fn spend_day(now: u64) -> String {
    timada_core::civil::format_date(now)
}

/// Unix seconds of the midnight (UTC) that ends the day `now` falls on:
/// when a day's cap frees.
pub fn spend_day_ends(now: u64) -> u64 {
    (now / 86_400 + 1) * 86_400
}

/// What the keyword source may be paid in a day (UTC), in millionths of a
/// US dollar: for one user's calls, and for everyone's together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpendCaps {
    pub user_usd_micros: u64,
    pub platform_usd_micros: u64,
}

/// Which of the day's caps a call would pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendCap {
    /// The user's own.
    User,
    /// The platform's, over every user.
    Platform,
}

/// Where the fetch of a seed stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FetchStatus {
    /// Never asked for.
    #[default]
    Never,
    /// Asked for; no node took it on yet.
    Requested,
    /// A node is asking the source.
    Fetching,
    /// The last fetch gave its rows.
    Fetched,
    /// The last fetch gave nothing.
    Failed,
}

impl FetchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Requested => "requested",
            Self::Fetching => "fetching",
            Self::Fetched => "fetched",
            Self::Failed => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_is_held_as_a_search_engine_reads_it() {
        assert_eq!(normalize_seed("  Arbre   à CHAT ").unwrap(), "arbre à chat");
        assert!(matches!(
            normalize_seed("   "),
            Err(ResearchError::InvalidSeed)
        ));
        assert!(matches!(
            normalize_seed(&"a".repeat(MAX_SEED_LEN + 1)),
            Err(ResearchError::InvalidSeed)
        ));
        assert_eq!(
            normalize_seed(&"é".repeat(MAX_SEED_LEN))
                .unwrap()
                .chars()
                .count(),
            MAX_SEED_LEN,
            "counted in characters, not bytes"
        );
    }

    #[test]
    fn a_site_is_its_host_without_www() {
        assert_eq!(normalize_domain(" WWW.Zooplus.fr. ").unwrap(), "zooplus.fr");
        assert_eq!(
            normalize_domain("fr.aliexpress.com").unwrap(),
            "fr.aliexpress.com"
        );
        assert_eq!(
            normalize_domain("étoile-du-chat.fr").unwrap(),
            "étoile-du-chat.fr"
        );
        for domain in [
            "",
            "localhost",
            "www.",
            "a..fr",
            "-a.fr",
            "a b.fr",
            "https://zooplus.fr",
            "zooplus.fr/chats",
        ] {
            assert!(normalize_domain(domain).is_err(), "{domain:?}");
        }
        assert!(normalize_domain(&format!("{}.fr", "a".repeat(MAX_DOMAIN_LEN))).is_err());
    }

    #[test]
    fn the_seeds_of_a_study_are_one_to_twenty_each_once() {
        assert_eq!(
            normalize_seeds(&["Arbre à chat", "", " arbre  à chat", "griffoir"]).unwrap(),
            ["arbre à chat", "griffoir"]
        );
        assert!(matches!(
            normalize_seeds(&["", "  "]),
            Err(ResearchError::NoSeed)
        ));
        let many: Vec<String> = (0..=MAX_SEEDS).map(|n| format!("seed {n}")).collect();
        assert!(matches!(
            normalize_seeds(&many),
            Err(ResearchError::TooManySeeds)
        ));
    }

    #[test]
    fn a_place_is_a_country_and_a_language_code() {
        assert_eq!(normalize_country(" fr ").unwrap(), "FR");
        assert_eq!(normalize_language("FR").unwrap(), "fr");
        assert_eq!(normalize_language("zh-TW").unwrap(), "zh-tw");
        for country in ["", "F", "FRA", "F1", "é"] {
            assert!(normalize_country(country).is_err(), "{country:?}");
        }
        for language in ["", "f", "french", "fr-", "fr_FR", "1a", "fr-françai"] {
            assert!(normalize_language(language).is_err(), "{language:?}");
        }
    }

    #[test]
    fn a_day_of_spending_ends_at_midnight_utc() {
        // 2023-11-14 22:13:20 UTC.
        let now = 1_700_000_000;
        assert_eq!(spend_day(now), "2023-11-14");
        let ends = spend_day_ends(now);
        assert_eq!(ends, 1_700_006_400);
        assert_eq!(spend_day(ends - 1), "2023-11-14");
        assert_eq!(spend_day(ends), "2023-11-15");
        assert_eq!(spend_day_ends(ends), ends + 86_400);
    }

    #[test]
    fn a_name_is_one_to_eighty_characters() {
        assert_eq!(
            normalize_name("  Arbres  à chat ").unwrap(),
            "Arbres à chat"
        );
        assert!(normalize_name(" ").is_err());
        assert!(normalize_name(&"n".repeat(MAX_NAME_LEN + 1)).is_err());
    }
}
