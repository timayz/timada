//! What Search Console says of a shop's site, a day at a time, and where
//! the daily look for new days stands.
//!
//! A day's numbers are one event on a stream of the day's own
//! ([`search_day_id`]): Google does not change a day it has finished
//! counting, so the stream is written once, and two nodes that both fetched
//! the day record it once. The rows live in the event, so a rebuilt read
//! model and every node of a cluster hold the same.
//!
//! The look itself leaves one small event a pass on the shop's watch
//! ([`search_watch_id`]): it found what there was, or why it could not.
//! When the next one is due follows from it, so a pass that fails is tried
//! again later, further apart each time.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{
    DayFetched, PageQueryRow, SearchCheckFailed, SearchChecked, SearchRow, SearchWatch,
};
use crate::error::GoogleError;
use crate::value_object::{
    CATCH_UP_SECS, CHECK_EVERY_SECS, MAX_PAGE_QUERY_ROWS, MAX_REASON_LEN, MAX_SEARCH_KEY_LEN,
    MAX_SEARCH_ROWS, normalize_site, retry_delay_secs, search_day_id, search_watch_id,
};

use super::Command;

/// How many times a pass's outcome is written again when another node
/// wrote the watch at the same moment.
const WATCH_ATTEMPTS: usize = 5;

/// What a site did over a day, or what one of its queries or pages did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchNumbers {
    pub clicks: u64,
    pub impressions: u64,
    /// The average position, in thousandths: 3,400 is position 3.4.
    pub position_milli: u32,
}

/// One finished day of a site, as Search Console gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayReport {
    /// `2026-11-20`, a day of Google's (Pacific time).
    pub day: String,
    /// The site's own numbers: the rows below add up to less (Google
    /// withholds rare queries) or to more (a search may show two pages).
    pub numbers: SearchNumbers,
    /// The most clicked queries, the most clicked first.
    pub queries: Vec<SearchRow>,
    /// The most clicked pages, by address.
    pub pages: Vec<SearchRow>,
    /// The queries each page showed for, in any order.
    pub page_queries: Vec<PageQueryRow>,
}

/// The write model of a shop's watch: where the look for new days stands.
#[evento::projection(id = id, name = "timada-google/SearchWatchState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct SearchWatchState {
    pub id: String,
    /// The shop.
    pub tenant_id: String,
    /// The site last looked at.
    pub site: String,
    /// Whether the last pass that worked left days to fetch.
    pub behind: bool,
    /// Unix seconds of the last pass that worked; 0 before any did.
    pub checked_at: u64,
    /// How many passes failed in a row since.
    pub attempts: u32,
    /// Why the last one failed; `None` once one works.
    pub last_error: Option<String>,
    /// Unix seconds from which the next pass is due.
    pub next_at: u64,
}

impl SearchWatchState {
    /// Whether a pass over `site` is due at `now`: another site than the
    /// one last looked at is due at once.
    pub fn is_due(&self, site: &str, now: u64) -> bool {
        self.site != site || now >= self.next_at
    }
}

pub fn search_watch_projection<E: Executor>() -> Projection<E, SearchWatchState> {
    Projection::new::<SearchWatch>()
        .handler(on_search_checked())
        .handler(on_search_check_failed())
        .strict()
}

#[evento::handler]
async fn on_search_checked(
    event: Event<SearchChecked>,
    watch: &mut SearchWatchState,
) -> anyhow::Result<()> {
    watch.id = event.aggregate_id.to_owned();
    watch.tenant_id = event.data.tenant_id.to_owned();
    watch.site = event.data.site.to_owned();
    watch.behind = event.data.behind;
    watch.checked_at = event.timestamp;
    watch.attempts = 0;
    watch.last_error = None;
    watch.next_at = event.timestamp
        + if event.data.behind {
            CATCH_UP_SECS
        } else {
            CHECK_EVERY_SECS
        };
    Ok(())
}

#[evento::handler]
async fn on_search_check_failed(
    event: Event<SearchCheckFailed>,
    watch: &mut SearchWatchState,
) -> anyhow::Result<()> {
    watch.id = event.aggregate_id.to_owned();
    watch.tenant_id = event.data.tenant_id.to_owned();
    // A first failure on another site starts from nothing.
    if watch.site != event.data.site {
        watch.behind = false;
        watch.checked_at = 0;
    }
    watch.site = event.data.site.to_owned();
    watch.attempts = event.data.attempts;
    watch.last_error = Some(event.data.reason.to_owned());
    watch.next_at = event.timestamp + retry_delay_secs(event.data.attempts);
    Ok(())
}

/// The rows of a day as they are kept: without the ones with no key or one
/// too long to be a query or an address, [`MAX_SEARCH_ROWS`] at most.
fn kept(rows: Vec<SearchRow>) -> Vec<SearchRow> {
    rows.into_iter()
        .filter(|row| !row.key.is_empty() && row.key.len() <= MAX_SEARCH_KEY_LEN)
        .take(MAX_SEARCH_ROWS)
        .collect()
}

/// The pairs of a day as they are kept: without the ones with no page or
/// no query, or one too long, the most shown first (then the most clicked,
/// then by page and query, so two nodes keep the same),
/// [`MAX_PAGE_QUERY_ROWS`] at most.
fn kept_page_queries(rows: Vec<PageQueryRow>) -> Vec<PageQueryRow> {
    let whole = |key: &str| !key.is_empty() && key.len() <= MAX_SEARCH_KEY_LEN;
    let mut rows: Vec<PageQueryRow> = rows
        .into_iter()
        .filter(|row| whole(&row.page) && whole(&row.query))
        .collect();
    rows.sort_by(|a, b| {
        (b.impressions, b.clicks)
            .cmp(&(a.impressions, a.clicks))
            .then_with(|| (&a.page, &a.query).cmp(&(&b.page, &b.query)))
    });
    rows.truncate(MAX_PAGE_QUERY_ROWS);
    rows
}

/// A day as `YYYY-MM-DD`.
fn is_day(day: &str) -> bool {
    day.len() == 10 && timada_core::civil::parse_date(day).is_some()
}

impl<E: Executor> Command<E> {
    /// Where the look for a shop's new days stands; `None` before its
    /// first pass.
    pub async fn load_search_watch(
        &self,
        tenant_id: &str,
    ) -> anyhow::Result<Option<SearchWatchState>> {
        search_watch_projection()
            .load(search_watch_id(tenant_id))
            .execute(&self.0)
            .await
    }

    /// Writes one event to the shop's watch, made from the watch as it
    /// stands; again when another node wrote it meanwhile.
    async fn write_watch<D, F>(
        &self,
        tenant_id: &str,
        routing_key: Option<String>,
        event: F,
    ) -> Result<(), GoogleError>
    where
        D: evento::AggregateEvent + bitcode::Encode,
        F: Fn(Option<&SearchWatchState>) -> D,
    {
        for _ in 0..WATCH_ATTEMPTS {
            let loaded = self.load_search_watch(tenant_id).await?;
            let mut write = match &loaded {
                Some(watch) => watch.write()?,
                None => {
                    let mut write = evento::append(search_watch_id(tenant_id));
                    write.original_version(0);
                    write
                }
            };
            write
                .routing_key_opt(routing_key.clone())
                .event(&event(loaded.as_ref()));
            match write.commit(&self.0).await {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Records one finished day of the shop's site. Whether it was
    /// written: a day recorded already (by another node, or an earlier
    /// pass) stands as it is.
    pub async fn record_search_day(
        &self,
        actor: &str,
        tenant_id: &str,
        site: &str,
        report: DayReport,
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        let site = normalize_site(site)?;
        if !is_day(&report.day) {
            return Err(GoogleError::InvalidDay);
        }
        let mut write = evento::append(search_day_id(tenant_id, &site, &report.day));
        write
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&DayFetched {
                tenant_id: tenant_id.to_owned(),
                site,
                day: report.day,
                clicks: report.numbers.clicks,
                impressions: report.numbers.impressions,
                position_milli: report.numbers.position_milli,
                queries: kept(report.queries),
                pages: kept(report.pages),
                page_queries: kept_page_queries(report.page_queries),
                fetched_by: actor.to_owned(),
            });
        match write.commit(&self.0).await {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Records that a pass over the shop's site found what there was;
    /// `behind` when older days are left for the next one, which is then
    /// due sooner.
    pub async fn record_search_check(
        &self,
        actor: &str,
        tenant_id: &str,
        site: &str,
        behind: bool,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let site = normalize_site(site)?;
        self.write_watch(tenant_id, routing_key, |_| SearchChecked {
            tenant_id: tenant_id.to_owned(),
            site: site.clone(),
            behind,
            checked_by: actor.to_owned(),
        })
        .await
    }

    /// Records that a pass over the shop's site failed, and why: the next
    /// one waits longer after each failure in a row.
    pub async fn record_search_failure(
        &self,
        actor: &str,
        tenant_id: &str,
        site: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let site = normalize_site(site)?;
        let reason: String = reason.trim().chars().take(MAX_REASON_LEN).collect();
        self.write_watch(tenant_id, routing_key, |watch| SearchCheckFailed {
            tenant_id: tenant_id.to_owned(),
            site: site.clone(),
            reason: reason.clone(),
            attempts: watch
                .filter(|watch| watch.site == site)
                .map_or(0, |watch| watch.attempts)
                .saturating_add(1),
            failed_by: actor.to_owned(),
        })
        .await
    }
}
