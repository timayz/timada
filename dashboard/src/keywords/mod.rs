//! Keyword data: how often a query is searched, what a click on it costs,
//! how hard it is to rank for and what the searcher is after. Choosing a
//! niche and building a shop's categories both start from these numbers.
//!
//! [`KeywordSource`] hides who the numbers come from; [`dataforseo`] is the
//! one source there is, set in the configuration's `[dataforseo]` section.
//! Without it there is no source and nothing here calls anyone.
//!
//! Every call costs money, so an answer carries what it cost for a cap to
//! count, and a failure says which kind it is ([`KeywordError`]): asking
//! again may do, or it will not.

pub mod dataforseo;
#[cfg(test)]
pub mod fake;

use std::sync::Arc;

use crate::config::DataforseoConfig;

/// How many keywords one call returns at most.
pub const MAX_ROWS: usize = 1000;

/// What the searcher of a keyword is after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// To learn something.
    Informational,
    /// To reach a site they already know.
    Navigational,
    /// To compare before buying.
    Commercial,
    /// To buy.
    Transactional,
}

impl Intent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Informational => "informational",
            Self::Navigational => "navigational",
            Self::Commercial => "commercial",
            Self::Transactional => "transactional",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "informational" => Some(Self::Informational),
            "navigational" => Some(Self::Navigational),
            "commercial" => Some(Self::Commercial),
            "transactional" => Some(Self::Transactional),
            _ => None,
        }
    }
}

/// One keyword as the source knows it. A figure the source has nothing for
/// is `None`, never a zero: a keyword nobody bids on has no cost per click,
/// which is not a click that costs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyword {
    pub keyword: String,
    /// Searches a month, averaged over the last twelve.
    pub volume: Option<u64>,
    /// What advertisers pay for a click on average, in US cents: the source
    /// prices clicks in dollars whatever the country.
    pub cpc_usd_cents: Option<u64>,
    /// How hard the first page is to reach, 0 (easy) to 100.
    pub difficulty: Option<u8>,
    pub intent: Option<Intent>,
}

/// The keywords a seed gave, and what the call cost.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Suggestions {
    /// The seed itself first when the source knows it, then the keywords
    /// that contain its words, the most searched first; [`MAX_ROWS`] at
    /// most.
    pub rows: Vec<Keyword>,
    /// What the source charged for the call, in millionths of a US dollar:
    /// a call costs about a cent and a keyword a hundredth of one.
    pub cost_usd_micros: u64,
}

/// A language the source has keywords in, for a country.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// Two letters, lowercase (`fr`).
    pub code: String,
    /// In English, as the source names it.
    pub name: String,
}

/// A country the source has keywords for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Two letters, uppercase (`FR`).
    pub country: String,
    /// In English, as the source names it.
    pub name: String,
    pub languages: Vec<Language>,
}

/// Why a call gave nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeywordError {
    /// The source could not answer (no answer in time, its own failure,
    /// asked too often): asking again later may do.
    Unavailable(String),
    /// The source will say the same again: the login is wrong, the balance
    /// is spent, it has nothing for this country and language. In its words.
    Refused(String),
}

impl std::fmt::Display for KeywordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "the keyword source is unavailable: {reason}"),
            Self::Refused(reason) => write!(f, "the keyword source refused: {reason}"),
        }
    }
}

impl std::error::Error for KeywordError {}

#[async_trait::async_trait]
pub trait KeywordSource: Send + Sync {
    /// The keywords that contain the words of `seed`, in any order, as they
    /// are searched in `country` (`FR`) in `language` (`fr`). One paid
    /// call; a seed nobody searches gives no rows and still costs.
    async fn suggestions(
        &self,
        seed: &str,
        country: &str,
        language: &str,
    ) -> Result<Suggestions, KeywordError>;

    /// The countries the source has keywords for, by name, each with its
    /// languages: what a form may offer. Free, and kept between calls.
    async fn places(&self) -> Result<Vec<Place>, KeywordError>;
}

/// The source the configuration sets; `None` without a `[dataforseo]`
/// login: keyword research is off.
pub fn source(config: &DataforseoConfig) -> Option<Arc<dyn KeywordSource>> {
    let (login, password) = config.credentials()?;
    Some(Arc::new(dataforseo::DataforseoClient::new(
        login,
        password,
        config.api_url.clone(),
    )))
}
