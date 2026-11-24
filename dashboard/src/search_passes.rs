//! Reads what Search Console says of the connected shops' sites, once a
//! day each: clicks, impressions and average position, for the site, by
//! query and by page, for the days Google has finished counting.
//!
//! Nothing in the event store fires later, so every node sweeps, on a
//! timer, the shops the read model says are due, each node those of its
//! part of the shops ([`Share`]). The shop's own streams decide: its
//! connection says whether Google can be read and for which site, and its
//! watch from when a pass is due. Two nodes that disagree for a moment on
//! who is up both ask Google, and the day is recorded once: it has a stream
//! of its own, written once.
//!
//! A pass asks Google three kinds of question, each as cheap as Google
//! makes them (its load limit counts grouping by query or by page as
//! costly, both at once as the most, and a long range as more):
//!
//! - the site's numbers by day over the days it may be missing: one
//!   question, no grouping by query or page, which also tells which days
//!   are finished (Google answers with those only);
//! - for each missing day, its most clicked queries, then its most clicked
//!   pages: two questions over one day each;
//! - for each missing day, which page showed for which query
//!   ([`PAGE_QUERIES_ASKED`] pairs, of which the most shown are kept): the
//!   costly grouping, which nothing else tells (a category's best query,
//!   the queries no category answers), so it is asked once, over one day.
//!
//! A shop just connected is owed [`BACKFILL_DAYS`] days, fetched the newest
//! first, [`DAYS_A_PASS`] a pass, with the next pass a few minutes later
//! while older days are left. A pass that fails is recorded with its reason
//! and tried again by a later one, further apart each time; the days it
//! recorded before failing stand. A grant Google no longer honours is
//! recorded on the connection (`crate::google`): the shop is not due again
//! until its member connects again.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use timada_core::Share;
use timada_core::civil::format_date;
use timada_google::{
    ACCESS_SEARCH_CONSOLE, BACKFILL_DAYS, Command, ConnectionState, DayReport, DueSearch,
    MAX_SEARCH_ROWS, SearchWatchState, day_before, search_days_held, searches_due,
};
use tokio::task::JoinHandle;

use crate::cluster::Executor;
use crate::db::Db;
use crate::google::{Dimension, Google, GoogleFault};

/// Who the days and the passes say recorded them.
pub const READER: &str = "timada-dashboard";

/// How many due shops one read takes.
const PAGE: i64 = 200;

/// How many days one pass fetches: three questions a day.
pub const DAYS_A_PASS: usize = 10;

/// How many pairs of a page and a query a day is asked for. Google lists
/// them by clicks; the ones kept are the most shown, so more are asked for
/// than kept, in the one question.
pub const PAGE_QUERIES_ASKED: usize = 5_000;

/// How far back a caught-up shop is asked about: Google finishes a day two
/// to three days later, and suggests looking at the last ten.
const RECENT_DAYS: u32 = 10;

/// What ends a pass early.
enum Stopped {
    /// Google gave nothing: recorded on the watch, unless the grant lapsed.
    Google(GoogleFault),
    /// The store failed: the pass is tried again by the next sweep.
    Store(anyhow::Error),
}

impl From<GoogleFault> for Stopped {
    fn from(fault: GoogleFault) -> Self {
        Self::Google(fault)
    }
}

impl From<anyhow::Error> for Stopped {
    fn from(err: anyhow::Error) -> Self {
        Self::Store(err)
    }
}

/// The sweep, with what it needs outside any request.
pub struct SearchPasses {
    /// The read model: the shops due, and the days each holds.
    db: Db,
    /// The store, read from this node's file: the connections and the
    /// watches decide, the read model only says where to look.
    store: Executor,
    /// This node's place among the nodes up, as of now: which shops it
    /// reads for.
    place: Arc<dyn Fn() -> Share + Send + Sync>,
    google: Google,
}

impl SearchPasses {
    pub fn new(
        db: Db,
        store: Executor,
        place: Arc<dyn Fn() -> Share + Send + Sync>,
        google: Google,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            place,
            google,
        })
    }

    /// Sweeps every `every`, the first time right away. Nothing is swept
    /// while no Google application is set.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            if !self.google.enabled() {
                return;
            }
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                match self.sweep(crate::auth::now()).await {
                    Ok(0) => {}
                    Ok(days) => tracing::info!(days, "Search Console days read"),
                    Err(err) => tracing::error!(error = %err, "Search Console not swept"),
                }
            }
        })
    }

    /// Reads Search Console for the shops due at `now` (unix seconds) among
    /// this node's part; returns how many days were recorded. A shop that
    /// fails is logged and the others go on.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let share = (self.place)();
        let mut recorded = 0;
        let mut after = String::new();
        loop {
            let due = searches_due(&self.db, now as i64, &after, PAGE).await?;
            for row in due.iter().filter(|row| share.owns(&row.tenant_id)) {
                match self.run(row, now).await {
                    Ok(days) => recorded += days,
                    Err(err) => {
                        tracing::error!(tenant = %row.tenant_id, error = %err, "a shop's Search Console not read");
                    }
                }
            }
            match due.last() {
                Some(last) if due.len() as i64 == PAGE => after = last.tenant_id.clone(),
                _ => return Ok(recorded),
            }
        }
    }

    /// One shop's pass, if its streams say it is due; how many days it
    /// recorded.
    async fn run(&self, due: &DueSearch, now: u64) -> anyhow::Result<usize> {
        let tenant_id = due.tenant_id.as_str();
        let command = Command(self.store.clone());
        // The write models decide; the read model only said where to look.
        let Some(connection) = command.load_connection(tenant_id).await? else {
            return Ok(0);
        };
        let Some(site) = connection
            .site
            .clone()
            .filter(|_| connection.reads(ACCESS_SEARCH_CONSOLE))
        else {
            return Ok(0);
        };
        let watch = command.load_search_watch(tenant_id).await?;
        if watch
            .as_ref()
            .is_some_and(|watch| !watch.is_due(&site, now))
        {
            return Ok(0);
        }
        let shop = timada_tenancy::Command(self.store.clone())
            .load_tenant(tenant_id)
            .await?;
        if !shop.is_some_and(|shop| !shop.is_suspended()) {
            return Ok(0);
        }
        let mut recorded = 0;
        let passed = self
            .fetch(
                &command,
                &connection,
                &site,
                watch.as_ref(),
                now,
                &mut recorded,
            )
            .await;
        match passed {
            Ok(behind) => {
                command
                    .record_search_check(READER, tenant_id, &site, behind)
                    .await?;
                tracing::debug!(tenant = %tenant_id, days = recorded, behind, "Search Console read");
            }
            // Recorded on the connection, or nothing to read with.
            Err(Stopped::Google(GoogleFault::ConnectAgain | GoogleFault::Off)) => {}
            Err(Stopped::Google(fault)) => {
                tracing::warn!(tenant = %tenant_id, %fault, "Search Console gave nothing");
                command
                    .record_search_failure(READER, tenant_id, &site, &fault.to_string())
                    .await?;
            }
            Err(Stopped::Store(err)) => return Err(err),
        }
        Ok(recorded)
    }

    /// Asks Google which days it has finished, and fetches and records the
    /// missing ones, the newest first. Whether older days are left for the
    /// next pass.
    async fn fetch(
        &self,
        command: &Command<Executor>,
        connection: &ConnectionState,
        site: &str,
        watch: Option<&SearchWatchState>,
        now: u64,
        recorded: &mut usize,
    ) -> Result<bool, Stopped> {
        let tenant_id = connection.tenant_id.as_str();
        let today = format_date(now);
        let oldest = day_before(&today, BACKFILL_DAYS);
        let held = search_days_held(&self.db, tenant_id, site, &oldest).await?;
        // A shop that is up to date is asked about the last days only: from
        // its last day held, or further back when that one is recent.
        let caught_up =
            watch.is_some_and(|watch| watch.site == site && watch.checked_at > 0 && !watch.behind);
        let first = match held.last() {
            Some(last) if caught_up => day_before(&today, RECENT_DAYS)
                .min(last.clone())
                .max(oldest.clone()),
            _ => oldest.clone(),
        };
        let google = &self.google;
        let days = google
            .read(&self.store, connection, |access| {
                let (first, today) = (first.clone(), today.clone());
                async move { google.search_days(&access, site, &first, &today).await }
            })
            .await?;
        let held: HashSet<&str> = held.iter().map(String::as_str).collect();
        let mut missing: Vec<_> = days
            .into_iter()
            .filter(|day| day.day >= oldest && !held.contains(day.day.as_str()))
            .collect();
        missing.sort_by(|a, b| b.day.cmp(&a.day));
        let behind = missing.len() > DAYS_A_PASS;
        for day in missing.into_iter().take(DAYS_A_PASS) {
            let rows = |dimension: Dimension| {
                let day = day.day.clone();
                google.read(&self.store, connection, move |access| {
                    let day = day.clone();
                    async move {
                        google
                            .search_rows(&access, site, &day, dimension, MAX_SEARCH_ROWS)
                            .await
                    }
                })
            };
            let queries = rows(Dimension::Query).await?;
            let pages = rows(Dimension::Page).await?;
            let page_queries = google
                .read(&self.store, connection, |access| {
                    let day = day.day.clone();
                    async move {
                        google
                            .search_page_queries(&access, site, &day, PAGE_QUERIES_ASKED)
                            .await
                    }
                })
                .await?;
            let written = command
                .record_search_day(
                    READER,
                    tenant_id,
                    site,
                    DayReport {
                        day: day.day,
                        numbers: day.numbers,
                        queries,
                        pages,
                        page_queries,
                    },
                )
                .await
                .map_err(anyhow::Error::from)?;
            if written {
                *recorded += 1;
            }
        }
        Ok(behind)
    }
}

#[cfg(test)]
mod tests;
