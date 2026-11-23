//! Queries over `research_keywords`, for a study's page.

use timada_core::db::{Db, FromRow};
use timada_core::page::{Cursor, Page, PageFrom, Side, read_page};

/// One row of `research_keywords`: a keyword as the last fetch of its seed
/// gave it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct KeywordRecord {
    pub study_id: String,
    pub id: String,
    /// The seed it came from.
    pub seed: String,
    pub keyword: String,
    /// Searches a month; `None` when the source has none.
    pub volume: Option<i64>,
    /// In US cents.
    pub cpc_usd_cents: Option<i64>,
    /// 0 (easy) to 100.
    pub difficulty: Option<i64>,
    pub intent: Option<String>,
    /// What the score made of it: `main`, `long_tail`, `variant`, the
    /// reason it was dropped, [`DUPLICATE`], or empty until it is scored.
    pub outcome: String,
    /// The keyword a variant counts under.
    pub variant_of: Option<String>,
    /// The volume, 0 without one: what the list is read by.
    pub sort_volume: i64,
    /// Unix seconds of the fetch that gave it.
    pub fetched_at: i64,
}

const KEYWORD_COLUMNS: &str = "SELECT study_id, id, seed, keyword, volume, cpc_usd_cents, \
     difficulty, intent, outcome, variant_of, sort_volume, fetched_at";

/// The outcome of a row whose keyword another seed gave too, with as many
/// searches or more: the keyword is scored once, on that other row.
pub const DUPLICATE: &str = "duplicate";

/// The statement of a page of the list: the rows on one side of a cursor
/// (`?2`, `?3`), down for the less searched ones, as the list reads, or up,
/// nearest first, off the index in its order.
pub(super) fn page_statement(down: bool) -> String {
    let tail = if down {
        "sort_volume <= ?2 AND (sort_volume < ?2 OR id < ?3)
         ORDER BY sort_volume DESC, id DESC LIMIT ?4"
    } else {
        "sort_volume >= ?2 AND (sort_volume > ?2 OR id > ?3)
         ORDER BY sort_volume ASC, id ASC LIMIT ?4"
    };
    format!(
        "{KEYWORD_COLUMNS} FROM research_keywords INDEXED BY research_keywords_volume
         WHERE study_id = ?1 AND {tail}"
    )
}

/// Where a keyword stands in the list.
fn cursor_of(row: &KeywordRecord) -> Cursor {
    Cursor {
        at: row.sort_volume,
        id: row.id.clone(),
    }
}

/// A page of a study's keywords, the most searched first, `limit` rows at
/// most (see [`timada_core::page`]: its "older" side is the less searched).
pub async fn keywords_of_study(
    db: &Db,
    study_id: &str,
    from: PageFrom<'_>,
    limit: i64,
) -> anyhow::Result<Page<KeywordRecord>> {
    let down = page_statement(true);
    let up = page_statement(false);
    let read = |side: Side, cursor: Option<Cursor>, n: i64| {
        let statement = match side {
            Side::Older => down.as_str(),
            Side::Newer => up.as_str(),
        };
        let (at, id) = cursor
            .map(|cursor| (cursor.at, cursor.id))
            .unwrap_or((i64::MAX, String::new()));
        db.fetch_all::<KeywordRecord>(statement, (study_id.to_owned(), at, id, n))
    };
    read_page(from, limit, read, cursor_of).await
}

/// The statement of a page of the keywords of one outcome (`?2`), on one
/// side of a cursor (`?3`, `?4`).
pub(super) fn outcome_statement(down: bool) -> String {
    let tail = if down {
        "sort_volume <= ?3 AND (sort_volume < ?3 OR id < ?4)
         ORDER BY sort_volume DESC, id DESC LIMIT ?5"
    } else {
        "sort_volume >= ?3 AND (sort_volume > ?3 OR id > ?4)
         ORDER BY sort_volume ASC, id ASC LIMIT ?5"
    };
    format!(
        "{KEYWORD_COLUMNS} FROM research_keywords INDEXED BY research_keywords_outcome
         WHERE study_id = ?1 AND outcome = ?2 AND {tail}"
    )
}

/// A page of the keywords of a study the score gave `outcome` to (a
/// [`Tier`](crate::Tier)'s or a [`DropReason`](crate::DropReason)'s
/// `as_str`, or [`VARIANT`](crate::VARIANT)), the most searched first.
pub async fn keywords_of_outcome(
    db: &Db,
    study_id: &str,
    outcome: &str,
    from: PageFrom<'_>,
    limit: i64,
) -> anyhow::Result<Page<KeywordRecord>> {
    let down = outcome_statement(true);
    let up = outcome_statement(false);
    let read = |side: Side, cursor: Option<Cursor>, n: i64| {
        let statement = match side {
            Side::Older => down.as_str(),
            Side::Newer => up.as_str(),
        };
        let (at, id) = cursor
            .map(|cursor| (cursor.at, cursor.id))
            .unwrap_or((i64::MAX, String::new()));
        db.fetch_all::<KeywordRecord>(
            statement,
            (study_id.to_owned(), outcome.to_owned(), at, id, n),
        )
    };
    read_page(from, limit, read, cursor_of).await
}

/// The statement of [`outcome_counts`].
pub(super) const OUTCOME_COUNTS: &str =
    "SELECT outcome, COUNT(*) FROM research_keywords INDEXED BY research_keywords_outcome
     WHERE study_id = ?1 GROUP BY outcome";

/// How many keywords of a study each outcome holds; one that holds none is
/// not listed.
pub async fn outcome_counts(db: &Db, study_id: &str) -> anyhow::Result<Vec<(String, i64)>> {
    db.fetch_all(OUTCOME_COUNTS, (study_id,)).await
}
