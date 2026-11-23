//! Queries over `research_fetches` and `research_spend`: where a seed was
//! last fetched, and what the keyword source was paid, by user.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

/// The last paid fetch of a seed in a country and a language.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct FetchRecord {
    /// The study whose stream holds the rows.
    pub study_id: String,
    /// Unix seconds.
    pub fetched_at: i64,
}

/// The statement of [`last_fetch`].
pub(super) const LAST_FETCH: &str = "SELECT study_id, fetched_at FROM research_fetches
     WHERE country = ?1 AND language = ?2 AND seed = ?3 AND fetched_at >= ?4";

/// The last paid fetch of `seed` as searched in `country` and `language`,
/// in any study, when it was made at `since` (unix seconds) or later.
pub async fn last_fetch(
    db: &Db,
    country: &str,
    language: &str,
    seed: &str,
    since: u64,
) -> Result<Option<FetchRecord>> {
    db.fetch_optional(LAST_FETCH, (country, language, seed, since as i64))
        .await
}

/// What one user's calls cost: on a day, and over the days of a month.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct SpendRecord {
    pub user_id: String,
    /// In millionths of a US dollar.
    pub day_usd_micros: i64,
    pub day_calls: i64,
    pub month_usd_micros: i64,
    pub month_calls: i64,
}

/// The statement of [`spend_by_user`].
pub(super) const SPEND_BY_USER: &str = "SELECT user_id,
        SUM(CASE WHEN day = ?1 THEN usd_micros ELSE 0 END) AS day_usd_micros,
        SUM(CASE WHEN day = ?1 THEN calls ELSE 0 END) AS day_calls,
        SUM(usd_micros) AS month_usd_micros,
        SUM(calls) AS month_calls
     FROM research_spend
     WHERE day >= ?2 AND day <= ?1
     GROUP BY user_id
     ORDER BY month_usd_micros DESC, user_id
     LIMIT ?3";

/// What each user's calls cost on `day` (`2026-11-23`) and since the first
/// of its month, the one who spent the most in the month first, `limit`
/// users at most.
pub async fn spend_by_user(db: &Db, day: &str, limit: i64) -> Result<Vec<SpendRecord>> {
    db.fetch_all(SPEND_BY_USER, (day, month_start(day), limit))
        .await
}

/// What every user's calls cost together on `day` and since the first of
/// its month, as `(day, month)` in millionths of a US dollar.
pub async fn spend_in_all(db: &Db, day: &str) -> Result<(i64, i64)> {
    db.fetch_one(
        "SELECT COALESCE(SUM(CASE WHEN day = ?1 THEN usd_micros ELSE 0 END), 0),
            COALESCE(SUM(usd_micros), 0)
         FROM research_spend WHERE day >= ?2 AND day <= ?1",
        (day, month_start(day)),
    )
    .await
}

/// The first day of the month of `day`: `2026-11-23` gives `2026-11-01`.
fn month_start(day: &str) -> String {
    format!("{}-01", day.get(..7).unwrap_or(day))
}
