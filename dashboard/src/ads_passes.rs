//! Reads what Google Ads says the connected shops' accounts cost and
//! brought, once a day each: cost, impressions, clicks, conversions and
//! their value, by campaign and device, by ad group, by product of the feed
//! and by search.
//!
//! Nothing in the event store fires later, so every node sweeps, on a
//! timer, the shops the read model says are due, each node those of its
//! part of the shops ([`Share`]). The shop's own streams decide: its
//! connection says whether Google Ads can be read and which account, and
//! its watch from when a pass is due. Two nodes that disagree for a moment
//! on who is up both ask, and a day read the same twice is recorded once.
//!
//! A pass asks four questions ([`AdsRead::ALL`]), each over a range of
//! days and each one operation of the platform's daily allowance at Google
//! Ads, however many rows come back. The allowance belongs to the Cloud
//! project ([`GoogleConfig::ads_calls_a_day`](crate::config::GoogleConfig)),
//! so every pass records how many questions it asked, and a sweep stops
//! once the day's passes, all shops and all nodes together, have used
//! [`PASSES_SHARE_PERCENT`] of it: the shops left stay due, and the pages
//! that list the accounts to choose from keep the rest.
//!
//! The days are the account's own (its time zone), up to yesterday there.
//! Google goes on counting a day after it ends (a conversion is counted on
//! the day of its click), so each pass reads the last
//! [`ADS_RECENT_DAYS`] again and records the ones whose numbers moved. A
//! shop that just chose its account is owed [`ADS_BACKFILL_DAYS`] days,
//! read the newest first, [`ADS_DAYS_A_PASS`] a pass, with the next pass a
//! few minutes later while older days are left. A pass that fails is
//! recorded with its reason and tried again later, further apart each time.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use timada_core::Share;
use timada_core::civil::format_date;
use timada_google::{
    ACCESS_ADS, ADS_BACKFILL_DAYS, ADS_DAYS_A_PASS, ADS_RECENT_DAYS, AdsAccount, AdsWatchState,
    Command, ConnectionState, DueAds, ads_calls_on, ads_due, day_before,
};
use tokio::task::JoinHandle;

use crate::cluster::Executor;
use crate::db::Db;
use crate::google::{AdsRead, AdsRows, Google, GoogleFault, assemble_ads_days};

/// Who the days and the passes say recorded them.
pub const READER: &str = "timada-dashboard";

/// How many due shops one read takes.
const PAGE: i64 = 200;

/// How much of the day's allowance the passes may use, in hundredths.
pub const PASSES_SHARE_PERCENT: u32 = 80;

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

/// The last day that is over in `time_zone` at `now` (unix seconds): the
/// account's yesterday. A zone the database does not name counts in UTC.
pub fn last_finished_day(now: u64, time_zone: &str) -> String {
    let zone = jiff::tz::TimeZone::get(time_zone).unwrap_or(jiff::tz::TimeZone::UTC);
    let today = i64::try_from(now)
        .ok()
        .and_then(|secs| jiff::Timestamp::from_second(secs).ok())
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(zone)
        .date();
    let day = today.yesterday().unwrap_or(today);
    format!("{:04}-{:02}-{:02}", day.year(), day.month(), day.day())
}

/// The days a pass reads, `(first, last, behind)`: `last_finished` is the
/// account's yesterday, and `watch` where its read stands, when it was
/// read before. `behind`: older days are left for the next pass.
pub fn days_to_read(last_finished: &str, watch: Option<&AdsWatchState>) -> (String, String, bool) {
    let oldest = day_before(last_finished, ADS_BACKFILL_DAYS - 1);
    let chunk = |last: &str| day_before(last, ADS_DAYS_A_PASS - 1).max(oldest.clone());
    match watch.filter(|watch| watch.checked_at > 0 && !watch.read_from.is_empty()) {
        // Older days are owed: the month before the oldest one read.
        Some(watch) if watch.behind && watch.read_from > oldest => {
            let last = day_before(&watch.read_from, 1);
            let first = chunk(&last);
            let behind = first > oldest;
            (first, last, behind)
        }
        // Up to date: the last days again, and the ones a node that was
        // away missed, a month at most.
        Some(watch) => {
            let recent = day_before(last_finished, ADS_RECENT_DAYS - 1);
            let first = recent.min(watch.read_to.clone()).max(chunk(last_finished));
            (first, last_finished.to_owned(), false)
        }
        // Never read: the last month, and the rest after.
        None => {
            let first = chunk(last_finished);
            let behind = first > oldest;
            (first, last_finished.to_owned(), behind)
        }
    }
}

/// The sweep, with what it needs outside any request.
pub struct AdsPasses {
    /// The read model: the shops due, and what the passes asked today.
    db: Db,
    /// The store, read from this node's file: the connections and the
    /// watches decide, the read model only says where to look.
    store: Executor,
    /// This node's place among the nodes up, as of now: which shops it
    /// reads for.
    place: Arc<dyn Fn() -> Share + Send + Sync>,
    google: Google,
    /// How many questions a day Google Ads takes from the platform.
    calls_a_day: u32,
}

impl AdsPasses {
    pub fn new(
        db: Db,
        store: Executor,
        place: Arc<dyn Fn() -> Share + Send + Sync>,
        google: Google,
        calls_a_day: u32,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            place,
            google,
            calls_a_day,
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
                    Ok(days) => tracing::info!(days, "Google Ads days read"),
                    Err(err) => tracing::error!(error = %err, "Google Ads not swept"),
                }
            }
        })
    }

    /// How many questions the passes may ask in a day.
    fn allowance(&self) -> i64 {
        i64::from(self.calls_a_day) * i64::from(PASSES_SHARE_PERCENT) / 100
    }

    /// Reads Google Ads for the shops due at `now` (unix seconds) among
    /// this node's part; returns how many days were recorded. A shop that
    /// fails is logged and the others go on.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let share = (self.place)();
        let pass = AdsRead::ALL.len() as i64;
        // What every node's passes asked today, as far as this node knows,
        // then what this sweep adds.
        let mut asked = ads_calls_on(&self.db, &format_date(now)).await?;
        let mut recorded = 0;
        let mut after = String::new();
        loop {
            let due = ads_due(&self.db, now as i64, &after, PAGE).await?;
            for row in due.iter().filter(|row| share.owns(&row.tenant_id)) {
                if asked + pass > self.allowance() {
                    tracing::warn!(
                        asked,
                        allowance = self.allowance(),
                        "the day's allowance at Google Ads is used: the shops left are read later"
                    );
                    return Ok(recorded);
                }
                match self.run(row, now).await {
                    Ok((days, calls)) => {
                        recorded += days;
                        asked += i64::from(calls);
                    }
                    Err(err) => {
                        tracing::error!(tenant = %row.tenant_id, error = %err, "a shop's Google Ads not read");
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
    /// recorded and how many questions it asked.
    async fn run(&self, due: &DueAds, now: u64) -> anyhow::Result<(usize, u32)> {
        let tenant_id = due.tenant_id.as_str();
        let command = Command(self.store.clone());
        // The write models decide; the read model only said where to look.
        let Some(connection) = command.load_connection(tenant_id).await? else {
            return Ok((0, 0));
        };
        let Some(account) = connection
            .ads
            .clone()
            .filter(|_| connection.reads(ACCESS_ADS))
        else {
            return Ok((0, 0));
        };
        let watch = command
            .load_ads_watch(tenant_id)
            .await?
            .filter(|watch| watch.customer_id == account.customer_id);
        if watch.as_ref().is_some_and(|watch| now < watch.next_at) {
            return Ok((0, 0));
        }
        let shop = timada_tenancy::Command(self.store.clone())
            .load_tenant(tenant_id)
            .await?;
        if !shop.is_some_and(|shop| !shop.is_suspended()) {
            return Ok((0, 0));
        }
        let (first, last, behind) =
            days_to_read(&last_finished_day(now, &account.time_zone), watch.as_ref());
        let calls = AtomicU32::new(0);
        let mut recorded = 0;
        let passed = self
            .fetch(
                &command,
                &connection,
                &account,
                (&first, &last),
                &calls,
                &mut recorded,
            )
            .await;
        let calls = calls.load(Ordering::Relaxed);
        let customer_id = account.customer_id.as_str();
        match passed {
            Ok(()) => {
                command
                    .record_ads_check(READER, tenant_id, customer_id, &first, &last, behind, calls)
                    .await?;
                tracing::debug!(tenant = %tenant_id, days = recorded, behind, "Google Ads read");
            }
            // Recorded on the connection, or nothing to read with.
            Err(Stopped::Google(GoogleFault::ConnectAgain | GoogleFault::Off)) => {}
            Err(Stopped::Google(fault)) => {
                tracing::warn!(tenant = %tenant_id, %fault, "Google Ads gave nothing");
                command
                    .record_ads_failure(READER, tenant_id, customer_id, &fault.to_string(), calls)
                    .await?;
            }
            Err(Stopped::Store(err)) => return Err(err),
        }
        Ok((recorded, calls))
    }

    /// Asks Google Ads its four reports over the days, and records each
    /// day whose numbers are new or moved.
    async fn fetch(
        &self,
        command: &Command<Executor>,
        connection: &ConnectionState,
        account: &AdsAccount,
        (first, last): (&str, &str),
        calls: &AtomicU32,
        recorded: &mut usize,
    ) -> Result<(), Stopped> {
        let google = &self.google;
        let mut rows = AdsRows::default();
        for what in AdsRead::ALL {
            let read = google
                .read(&self.store, connection, |access| async move {
                    calls.fetch_add(1, Ordering::Relaxed);
                    let mut rows = AdsRows::default();
                    google
                        .ads_report(&access, account, what, first, last, &mut rows)
                        .await?;
                    Ok(rows)
                })
                .await?;
            rows.currency = rows.currency.or(read.currency);
            rows.campaigns.extend(read.campaigns);
            rows.ad_groups.extend(read.ad_groups);
            rows.products.extend(read.products);
            rows.terms.extend(read.terms);
        }
        for day in assemble_ads_days(rows, &account.currency) {
            // Google answers within the days asked; anything else is not
            // this pass's to record.
            if day.day.as_str() < first || day.day.as_str() > last {
                continue;
            }
            let written = command
                .record_ads_day(READER, &connection.tenant_id, &account.customer_id, day)
                .await
                .map_err(anyhow::Error::from)?;
            if written {
                *recorded += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
