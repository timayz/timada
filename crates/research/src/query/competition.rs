//! Queries over `research_domains`, for a study's page: who holds the first
//! results of its main keywords.

use timada_core::db::{Db, FromRow};

use crate::score::Tier;
use crate::value_object::{MAX_CHECKED_KEYWORDS, MAX_RESULTS};

/// One row of `research_domains`: a site found in the first results of a
/// study's main keywords.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DomainRow {
    pub domain: String,
    /// For how many of the main keywords it was found.
    pub keywords: i64,
    /// Its best place among them, from 1.
    pub best: i64,
    /// Its place for each of those keywords, the best first: `1, 3, 3`.
    pub positions: String,
}

/// The statement of [`main_keywords`].
pub(super) const MAIN_KEYWORDS: &str =
    "SELECT keyword FROM research_keywords INDEXED BY research_keywords_outcome
     WHERE study_id = ?1 AND outcome = ?2 ORDER BY sort_volume DESC, id DESC LIMIT ?3";

/// The main keywords of a study whose first results are looked at: the
/// most searched, [`MAX_CHECKED_KEYWORDS`] at most.
pub async fn main_keywords(db: &Db, study_id: &str) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String,)> = db
        .fetch_all(
            MAIN_KEYWORDS,
            (study_id, Tier::Main.as_str(), MAX_CHECKED_KEYWORDS as i64),
        )
        .await?;
    Ok(rows.into_iter().map(|(keyword,)| keyword).collect())
}

/// The statement of [`domains_of_study`].
pub(super) const DOMAINS: &str = "SELECT domain, keywords, best, positions
     FROM research_domains INDEXED BY research_domains_listed
     WHERE study_id = ?1 ORDER BY keywords DESC, best ASC, domain ASC LIMIT ?2";

/// The sites found in the first results of a study's main keywords: first
/// the ones found for the most keywords, then the best placed. Every one of
/// them: the keywords looked at and the results kept of each bound the
/// list.
pub async fn domains_of_study(db: &Db, study_id: &str) -> anyhow::Result<Vec<DomainRow>> {
    db.fetch_all(
        DOMAINS,
        (study_id, (MAX_CHECKED_KEYWORDS * MAX_RESULTS) as i64),
    )
    .await
}
