//! Queries over the read model: which shops' passes are due, which days a
//! site holds, and what a period comes to.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

/// A shop whose daily pass is due, and the site to look at.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DueSearch {
    pub tenant_id: String,
    pub site: String,
}

/// The statement of [`searches_due`]. It walks the connections by shop:
/// one row a connected shop, each matched to its watch by key. A sweep
/// reads them all, a page at a time.
pub(super) const SEARCHES_DUE: &str = "SELECT c.tenant_id, c.site
     FROM google_connections c
     LEFT JOIN google_search_watch w ON w.tenant_id = c.tenant_id
     WHERE c.tenant_id > ?1 AND c.lapsed = 0
        AND (w.tenant_id IS NULL OR w.site <> c.site OR w.next_at <= ?2)
     ORDER BY c.tenant_id
     LIMIT ?3";

/// The shops whose pass is due at `now` (unix seconds), by id after
/// `after`, `limit` at most: connected, their grant honoured, a site
/// chosen, and never looked at, or not since the site changed, or due.
pub async fn searches_due(db: &Db, now: i64, after: &str, limit: i64) -> Result<Vec<DueSearch>> {
    db.fetch_all(SEARCHES_DUE, (after, now, limit)).await
}

/// The statement of [`search_days_held`].
pub(super) const DAYS_HELD: &str = "SELECT day FROM google_search_days
     WHERE tenant_id = ?1 AND site = ?2 AND day >= ?3 ORDER BY day";

/// The days of `site` the shop holds from `since` (`2026-08-20`) on, the
/// oldest first.
pub async fn search_days_held(
    db: &Db,
    tenant_id: &str,
    site: &str,
    since: &str,
) -> Result<Vec<String>> {
    let days: Vec<(String,)> = db.fetch_all(DAYS_HELD, (tenant_id, site, since)).await?;
    Ok(days.into_iter().map(|(day,)| day).collect())
}

/// The last day of `site` the shop holds; `None` before the first.
pub async fn last_search_day(db: &Db, tenant_id: &str, site: &str) -> Result<Option<String>> {
    let day: Option<(String,)> = db
        .fetch_optional(
            "SELECT day FROM google_search_days
             WHERE tenant_id = ?1 AND site = ?2 ORDER BY day DESC LIMIT 1",
            (tenant_id, site),
        )
        .await?;
    Ok(day.map(|(day,)| day))
}

/// The first day of `site` the shop holds: the first Google showed it on,
/// as far back as it was read; `None` before the first.
pub async fn first_search_day(db: &Db, tenant_id: &str, site: &str) -> Result<Option<String>> {
    let day: Option<(String,)> = db
        .fetch_optional(
            "SELECT day FROM google_search_days
             WHERE tenant_id = ?1 AND site = ?2 ORDER BY day LIMIT 1",
            (tenant_id, site),
        )
        .await?;
    Ok(day.map(|(day,)| day))
}

/// What a site, a query or a page did over a period.
#[derive(Debug, Clone, Copy, Default, FromRow, PartialEq, Eq)]
pub struct SearchFigures {
    pub clicks: i64,
    pub impressions: i64,
    /// Each day's position times its impressions, added up: the average
    /// position over the period is this over the impressions.
    pub weighted_position_milli: i64,
}

impl SearchFigures {
    /// Clicks per impression, in hundredths of a percent; `None` without
    /// an impression.
    pub fn click_rate_bps(&self) -> Option<i64> {
        (self.impressions > 0)
            .then(|| (self.clicks * 10_000 + self.impressions / 2) / self.impressions)
    }

    /// The average position, in tenths (34 is position 3.4), each day
    /// counting for its impressions; `None` without an impression.
    pub fn position_tenths(&self) -> Option<i64> {
        (self.impressions > 0).then(|| (self.weighted_position_milli / self.impressions + 50) / 100)
    }
}

/// A period's totals, and over how many days it holds numbers.
#[derive(Debug, Clone, Copy, Default, FromRow, PartialEq, Eq)]
pub struct SearchTotals {
    pub days: i64,
    pub clicks: i64,
    pub impressions: i64,
    pub weighted_position_milli: i64,
}

impl SearchTotals {
    pub fn figures(&self) -> SearchFigures {
        SearchFigures {
            clicks: self.clicks,
            impressions: self.impressions,
            weighted_position_milli: self.weighted_position_milli,
        }
    }
}

/// The statement of [`search_totals`].
pub(super) const TOTALS: &str = "SELECT COUNT(*) AS days,
        COALESCE(SUM(clicks), 0) AS clicks,
        COALESCE(SUM(impressions), 0) AS impressions,
        COALESCE(SUM(position_milli * impressions), 0) AS weighted_position_milli
     FROM google_search_days
     WHERE tenant_id = ?1 AND site = ?2 AND day >= ?3 AND day <= ?4";

/// What `site` did from `first` to `last` (`2026-11-20`), both included.
pub async fn search_totals(
    db: &Db,
    tenant_id: &str,
    site: &str,
    first: &str,
    last: &str,
) -> Result<SearchTotals> {
    db.fetch_one(TOTALS, (tenant_id, site, first, last)).await
}

/// A query or a page over a period.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct SearchLine {
    /// The query as typed, or the page's address.
    pub key: String,
    pub clicks: i64,
    pub impressions: i64,
    pub weighted_position_milli: i64,
}

impl SearchLine {
    pub fn figures(&self) -> SearchFigures {
        SearchFigures {
            clicks: self.clicks,
            impressions: self.impressions,
            weighted_position_milli: self.weighted_position_milli,
        }
    }
}

/// The statement of [`top_queries`] (`query`) and [`top_pages`] (`page`):
/// the period's rows are one range of the key, added up by query or page.
/// It reads every row of the period, [`MAX_SEARCH_ROWS`](crate::MAX_SEARCH_ROWS)
/// a day at most over [`KEPT_ROW_DAYS`](crate::KEPT_ROW_DAYS) days at most.
pub(super) fn top_statement(table: &str, column: &str) -> String {
    format!(
        "SELECT {column} AS key, SUM(clicks) AS clicks, SUM(impressions) AS impressions,
            SUM(position_milli * impressions) AS weighted_position_milli
         FROM {table}
         WHERE tenant_id = ?1 AND site = ?2 AND day >= ?3 AND day <= ?4
         GROUP BY {column}
         ORDER BY clicks DESC, impressions DESC, {column}
         LIMIT ?5"
    )
}

/// The queries `site` showed for from `first` to `last`, the most clicked
/// first, then the most shown, `limit` at most.
pub async fn top_queries(
    db: &Db,
    tenant_id: &str,
    site: &str,
    first: &str,
    last: &str,
    limit: i64,
) -> Result<Vec<SearchLine>> {
    db.fetch_all(
        &top_statement("google_search_queries", "query"),
        (tenant_id, site, first, last, limit),
    )
    .await
}

/// The pages of `site` found from `first` to `last`, the most clicked
/// first, then the most shown, `limit` at most.
pub async fn top_pages(
    db: &Db,
    tenant_id: &str,
    site: &str,
    first: &str,
    last: &str,
    limit: i64,
) -> Result<Vec<SearchLine>> {
    db.fetch_all(
        &top_statement("google_search_pages", "page"),
        (tenant_id, site, first, last, limit),
    )
    .await
}

/// A query a page showed for over a period.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct PageQueryLine {
    /// The page's address.
    pub page: String,
    /// The query as typed.
    pub query: String,
    pub clicks: i64,
    pub impressions: i64,
    pub weighted_position_milli: i64,
}

impl PageQueryLine {
    pub fn figures(&self) -> SearchFigures {
        SearchFigures {
            clicks: self.clicks,
            impressions: self.impressions,
            weighted_position_milli: self.weighted_position_milli,
        }
    }
}

/// The statement of [`page_queries`]: the period's rows are one range of
/// the key, added up by page and query. It reads every row of the period,
/// [`MAX_PAGE_QUERY_ROWS`](crate::MAX_PAGE_QUERY_ROWS) a day at most.
pub(super) const PAGE_QUERIES: &str = "SELECT page, query, SUM(clicks) AS clicks,
        SUM(impressions) AS impressions,
        SUM(position_milli * impressions) AS weighted_position_milli
     FROM google_search_page_queries
     WHERE tenant_id = ?1 AND site = ?2 AND day >= ?3 AND day <= ?4
     GROUP BY page, query
     ORDER BY impressions DESC, page, query
     LIMIT ?5";

/// The queries each page of `site` showed for from `first` to `last`, the
/// most shown first, `limit` at most.
pub async fn page_queries(
    db: &Db,
    tenant_id: &str,
    site: &str,
    first: &str,
    last: &str,
    limit: i64,
) -> Result<Vec<PageQueryLine>> {
    db.fetch_all(PAGE_QUERIES, (tenant_id, site, first, last, limit))
        .await
}
