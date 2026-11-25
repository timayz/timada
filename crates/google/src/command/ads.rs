//! What Google Ads says a shop's account cost and brought, a day at a
//! time, and where the daily read stands.
//!
//! A day's numbers are an event on a stream of the day's own
//! ([`ads_day_id`]). Unlike a day of Search Console, Google goes on
//! counting one after it ends: a conversion is counted on the day of the
//! click that led to it, and may come weeks later. So the last days are
//! read again by every pass, and a day whose numbers moved gets another
//! event, which stands for the one before; a day read the same is left
//! alone, so the stream grows by the changes only. The rows live in the
//! event, so a rebuilt read model and every node of a cluster hold the
//! same.
//!
//! The read itself leaves one small event a pass on the shop's watch
//! ([`ads_watch_id`]): the days it read and how many questions that took,
//! or why it could not. When the next one is due follows from it, and so
//! does what the platform has used of its daily allowance at Google Ads.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{
    AdsAdGroupRow, AdsCampaignRow, AdsCheckFailed, AdsChecked, AdsDay, AdsDayFetched, AdsNumbers,
    AdsProductRow, AdsTermRow, AdsWatch,
};
use crate::error::GoogleError;
use crate::value_object::{
    ADS_DEVICES, CATCH_UP_SECS, CHECK_EVERY_SECS, MAX_ADS_AD_GROUP_ROWS, MAX_ADS_CAMPAIGN_ROWS,
    MAX_ADS_NAME_LEN, MAX_ADS_PRODUCT_ROWS, MAX_ADS_TERM_ROWS, MAX_REASON_LEN, MAX_SEARCH_KEY_LEN,
    ads_day_id, ads_watch_id, normalize_customer_id, retry_delay_secs,
};

use super::Command;

/// How many times a pass's outcome, or a day, is written again when
/// another node wrote the same stream at the same moment.
const ATTEMPTS: usize = 5;

/// One day of an account, as Google Ads gave it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdsDayReport {
    /// `2026-11-20`, a day of the account's time zone.
    pub day: String,
    /// The account's currency (`EUR`).
    pub currency: String,
    pub campaigns: Vec<AdsCampaignRow>,
    pub ad_groups: Vec<AdsAdGroupRow>,
    pub products: Vec<AdsProductRow>,
    pub terms: Vec<AdsTermRow>,
}

impl AdsNumbers {
    /// Whether nothing was shown, clicked or counted.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// What sorts the rows that cost the most first.
    fn weight(&self) -> (u64, u64, u64) {
        (self.cost_micros, self.clicks, self.impressions)
    }
}

/// A name short enough to keep.
fn named(name: &str) -> String {
    name.trim().chars().take(MAX_ADS_NAME_LEN).collect()
}

/// Whether `id` is one of Google Ads' own: digits.
fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.chars().all(|c| c.is_ascii_digit())
}

impl AdsDayReport {
    /// The day as it is kept: without the rows that say nothing or name
    /// nothing, the ones that cost the most first (then by what they name,
    /// so two nodes keep the same), each list cut to what a day keeps.
    fn kept(mut self) -> Self {
        self.currency = self.currency.trim().to_ascii_uppercase();
        self.campaigns.retain(|row| {
            is_id(&row.campaign_id)
                && ADS_DEVICES.contains(&row.device.as_str())
                && !row.numbers.is_empty()
        });
        for row in &mut self.campaigns {
            row.name = named(&row.name);
        }
        self.campaigns.sort_by(|a, b| {
            b.numbers
                .weight()
                .cmp(&a.numbers.weight())
                .then_with(|| (&a.campaign_id, &a.device).cmp(&(&b.campaign_id, &b.device)))
        });
        self.campaigns
            .dedup_by(|b, a| (&a.campaign_id, &a.device) == (&b.campaign_id, &b.device));
        self.campaigns.truncate(MAX_ADS_CAMPAIGN_ROWS);

        self.ad_groups.retain(|row| {
            is_id(&row.campaign_id) && is_id(&row.ad_group_id) && !row.numbers.is_empty()
        });
        for row in &mut self.ad_groups {
            row.name = named(&row.name);
        }
        self.ad_groups.sort_by(|a, b| {
            b.numbers
                .weight()
                .cmp(&a.numbers.weight())
                .then_with(|| a.ad_group_id.cmp(&b.ad_group_id))
        });
        self.ad_groups
            .dedup_by(|b, a| a.ad_group_id == b.ad_group_id);
        self.ad_groups.truncate(MAX_ADS_AD_GROUP_ROWS);

        self.products.retain(|row| {
            !row.item_id.is_empty()
                && row.item_id.len() <= MAX_ADS_NAME_LEN
                && is_id(&row.campaign_id)
                && is_id(&row.ad_group_id)
                && !row.numbers.is_empty()
        });
        for row in &mut self.products {
            // Google Ads reports an item's id in lower case; said here so
            // that nothing depends on it doing so.
            row.item_id = row.item_id.to_lowercase();
        }
        self.products.sort_by(|a, b| {
            b.numbers
                .weight()
                .cmp(&a.numbers.weight())
                .then_with(|| (&a.item_id, &a.ad_group_id).cmp(&(&b.item_id, &b.ad_group_id)))
        });
        self.products
            .dedup_by(|b, a| (&a.item_id, &a.ad_group_id) == (&b.item_id, &b.ad_group_id));
        self.products.truncate(MAX_ADS_PRODUCT_ROWS);

        self.terms.retain(|row| {
            !row.term.is_empty()
                && row.term.len() <= MAX_SEARCH_KEY_LEN
                && is_id(&row.campaign_id)
                && is_id(&row.ad_group_id)
                && !row.numbers.is_empty()
        });
        self.terms.sort_by(|a, b| {
            b.numbers
                .weight()
                .cmp(&a.numbers.weight())
                .then_with(|| (&a.term, &a.ad_group_id).cmp(&(&b.term, &b.ad_group_id)))
        });
        self.terms
            .dedup_by(|b, a| (&a.term, &a.ad_group_id) == (&b.term, &b.ad_group_id));
        self.terms.truncate(MAX_ADS_TERM_ROWS);
        self
    }
}

/// The write model of a day: what the last read of it gave.
#[evento::projection(id = id, name = "timada-google/AdsDayState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct AdsDayState {
    pub id: String,
    pub last: AdsDayReport,
}

pub fn ads_day_projection<E: Executor>() -> Projection<E, AdsDayState> {
    Projection::new::<AdsDay>()
        .handler(on_ads_day_fetched())
        .strict()
}

#[evento::handler]
async fn on_ads_day_fetched(
    event: Event<AdsDayFetched>,
    day: &mut AdsDayState,
) -> anyhow::Result<()> {
    day.id = event.aggregate_id.to_owned();
    day.last = AdsDayReport {
        day: event.data.day.to_owned(),
        currency: event.data.currency.to_owned(),
        campaigns: event.data.campaigns.to_owned(),
        ad_groups: event.data.ad_groups.to_owned(),
        products: event.data.products.to_owned(),
        terms: event.data.terms.to_owned(),
    };
    Ok(())
}

/// The write model of a shop's watch: where the read of its account stands.
#[evento::projection(id = id, name = "timada-google/AdsWatchState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct AdsWatchState {
    pub id: String,
    /// The shop.
    pub tenant_id: String,
    /// The account last read.
    pub customer_id: String,
    /// Whether the last pass that worked left older days to read.
    pub behind: bool,
    /// The oldest day read of the account so far; empty before any was.
    pub read_from: String,
    /// The newest day read of the account so far.
    pub read_to: String,
    /// Unix seconds of the last pass that worked; 0 before any did.
    pub checked_at: u64,
    /// How many passes failed in a row since.
    pub attempts: u32,
    /// Why the last one failed; `None` once one works.
    pub last_error: Option<String>,
    /// Unix seconds from which the next pass is due.
    pub next_at: u64,
    /// The day (UTC) of the last pass, and how many questions the shop's
    /// passes asked Google Ads on it.
    pub calls_day: String,
    pub calls: u32,
}

impl AdsWatchState {
    /// Whether a pass over `customer_id` is due at `now`: another account
    /// than the one last read is due at once.
    pub fn is_due(&self, customer_id: &str, now: u64) -> bool {
        self.customer_id != customer_id || now >= self.next_at
    }

    /// Counts the questions of a pass made at `at` in the day's tally.
    fn count(&mut self, at: u64, calls: u32) {
        let day = timada_core::civil::format_date(at);
        if self.calls_day != day {
            self.calls_day = day;
            self.calls = 0;
        }
        self.calls = self.calls.saturating_add(calls);
    }
}

pub fn ads_watch_projection<E: Executor>() -> Projection<E, AdsWatchState> {
    Projection::new::<AdsWatch>()
        .handler(on_ads_checked())
        .handler(on_ads_check_failed())
        .strict()
}

#[evento::handler]
async fn on_ads_checked(event: Event<AdsChecked>, watch: &mut AdsWatchState) -> anyhow::Result<()> {
    watch.id = event.aggregate_id.to_owned();
    watch.tenant_id = event.data.tenant_id.to_owned();
    // Another account starts from nothing; the same one only ever grows
    // the days read, at both ends.
    if watch.customer_id != event.data.customer_id || watch.read_from.is_empty() {
        watch.read_from = event.data.first.to_owned();
        watch.read_to = event.data.last.to_owned();
    } else {
        if event.data.first < watch.read_from {
            watch.read_from = event.data.first.to_owned();
        }
        if event.data.last > watch.read_to {
            watch.read_to = event.data.last.to_owned();
        }
    }
    watch.customer_id = event.data.customer_id.to_owned();
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
    watch.count(event.timestamp, event.data.calls);
    Ok(())
}

#[evento::handler]
async fn on_ads_check_failed(
    event: Event<AdsCheckFailed>,
    watch: &mut AdsWatchState,
) -> anyhow::Result<()> {
    watch.id = event.aggregate_id.to_owned();
    watch.tenant_id = event.data.tenant_id.to_owned();
    // A first failure on another account starts from nothing.
    if watch.customer_id != event.data.customer_id {
        watch.behind = false;
        watch.checked_at = 0;
        watch.read_from = String::new();
        watch.read_to = String::new();
    }
    watch.customer_id = event.data.customer_id.to_owned();
    watch.attempts = event.data.attempts;
    watch.last_error = Some(event.data.reason.to_owned());
    watch.next_at = event.timestamp + retry_delay_secs(event.data.attempts);
    watch.count(event.timestamp, event.data.calls);
    Ok(())
}

/// A day as `YYYY-MM-DD`.
fn is_day(day: &str) -> bool {
    day.len() == 10 && timada_core::civil::parse_date(day).is_some()
}

impl<E: Executor> Command<E> {
    /// Where the read of a shop's Google Ads account stands; `None` before
    /// its first pass.
    pub async fn load_ads_watch(&self, tenant_id: &str) -> anyhow::Result<Option<AdsWatchState>> {
        ads_watch_projection()
            .load(ads_watch_id(tenant_id))
            .execute(&self.0)
            .await
    }

    /// Writes one event to the shop's watch, made from the watch as it
    /// stands; again when another node wrote it meanwhile.
    async fn write_ads_watch<D, F>(
        &self,
        tenant_id: &str,
        routing_key: Option<String>,
        event: F,
    ) -> Result<(), GoogleError>
    where
        D: evento::AggregateEvent + bitcode::Encode,
        F: Fn(Option<&AdsWatchState>) -> D,
    {
        for _ in 0..ATTEMPTS {
            let loaded = self.load_ads_watch(tenant_id).await?;
            let mut write = match &loaded {
                Some(watch) => watch.write()?,
                None => {
                    let mut write = evento::append(ads_watch_id(tenant_id));
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
    /// Records what Google Ads says of one day of the shop's account.
    /// Whether it was written: a day that reads the same as it was last
    /// recorded is left as it is.
    pub async fn record_ads_day(
        &self,
        actor: &str,
        tenant_id: &str,
        customer_id: &str,
        report: AdsDayReport,
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        let customer_id = normalize_customer_id(customer_id)?;
        if !is_day(&report.day) {
            return Err(GoogleError::InvalidDay);
        }
        let report = report.kept();
        let id = ads_day_id(tenant_id, &customer_id, &report.day);
        for _ in 0..ATTEMPTS {
            let loaded = ads_day_projection().load(&id).execute(&self.0).await?;
            let mut write = match &loaded {
                Some(day) if day.last == report => return Ok(false),
                Some(day) => day.write()?,
                None => {
                    let mut write = evento::append(&id);
                    write.original_version(0);
                    write
                }
            };
            write
                .routing_key_opt(routing_key.clone())
                .event(&AdsDayFetched {
                    tenant_id: tenant_id.to_owned(),
                    customer_id: customer_id.clone(),
                    day: report.day.clone(),
                    currency: report.currency.clone(),
                    campaigns: report.campaigns.clone(),
                    ad_groups: report.ad_groups.clone(),
                    products: report.products.clone(),
                    terms: report.terms.clone(),
                    fetched_by: actor.to_owned(),
                });
            match write.commit(&self.0).await {
                Ok(_) => return Ok(true),
                // Another node read the day at the same moment: decided
                // again against what it wrote.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// Records that a pass over the shop's account read the days from
    /// `first` to `last` with `calls` questions; `behind` when older days
    /// are left for the next one, which is then due sooner.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_ads_check(
        &self,
        actor: &str,
        tenant_id: &str,
        customer_id: &str,
        first: &str,
        last: &str,
        behind: bool,
        calls: u32,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let customer_id = normalize_customer_id(customer_id)?;
        if !is_day(first) || !is_day(last) || first > last {
            return Err(GoogleError::InvalidDay);
        }
        self.write_ads_watch(tenant_id, routing_key, |_| AdsChecked {
            tenant_id: tenant_id.to_owned(),
            customer_id: customer_id.clone(),
            first: first.to_owned(),
            last: last.to_owned(),
            behind,
            calls,
            checked_by: actor.to_owned(),
        })
        .await
    }

    /// Records that a pass over the shop's account failed after `calls`
    /// questions, and why: the next one waits longer after each failure in
    /// a row.
    pub async fn record_ads_failure(
        &self,
        actor: &str,
        tenant_id: &str,
        customer_id: &str,
        reason: &str,
        calls: u32,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let customer_id = normalize_customer_id(customer_id)?;
        let reason: String = reason.trim().chars().take(MAX_REASON_LEN).collect();
        self.write_ads_watch(tenant_id, routing_key, |watch| AdsCheckFailed {
            tenant_id: tenant_id.to_owned(),
            customer_id: customer_id.clone(),
            reason: reason.clone(),
            attempts: watch
                .filter(|watch| watch.customer_id == customer_id)
                .map_or(0, |watch| watch.attempts)
                .saturating_add(1),
            calls,
            failed_by: actor.to_owned(),
        })
        .await
    }
}
