//! A new shop's checkpoints: the numbers read on days 21, 45, 90 and 120
//! of its first campaign, and the notice that tells its owners.
//!
//! Nothing in the event store fires on a date, so a checkpoint is read
//! when the daily read of Google Ads ends for the shop (`AdsChecked`) and
//! its day has come: every shop with a campaign is read every day, and a
//! checkpoint read a few hours late reads the same numbers. The reading is
//! recorded on the shop's launch (`timada-launch`) with what it says
//! against the shop's thresholds; read by two nodes, it is recorded once.
//!
//! Each checkpoint reads what timada already holds:
//! - day 21, what Google Ads says the first campaign brought for what it
//!   cost over its first three weeks;
//! - day 45, what the shop's own orders that came from an ad sold for what
//!   the ads cost, against what those sales have to bring to break even;
//! - day 90, Search Console: the categories in the first positions, and
//!   whether the impressions rise;
//! - day 120, the margin left once goods, costs and ads are paid, every
//!   channel together.
//!
//! `CheckpointReached` owes the owners a mail, with the numbers; the
//! mailroom sends it once ([`timada_core::Sending`], `Notice`).

use std::collections::HashMap;
use std::sync::Arc;

use evento::{
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_google::aggregator::AdsChecked;
use timada_google::{
    ACCESS_SEARCH_CONSOLE, AdsAccount, CAMPAIGN_SHOPPING, CampaignState, Within, ads_totals,
    ads_within_campaign, day_before, last_search_day, return_of, search_totals, top_pages,
};
use timada_launch::aggregator::{CheckpointReached, Notice};
use timada_launch::{
    LAUNCH_ACTOR, LaunchState, NOTICE_WINDOW_SECS, Reading, Thresholds, checkpoint_at,
    checkpoint_due, notice_id, notice_projection,
};
use timada_orders::Period;
use topcoat::context::Cx;

use crate::app::t::slug::sales::period_of_days;
use crate::app::t::slug::search_console::landing::{Landing, Landings};
use crate::campaigns::{held, reach};
use crate::cluster::Executor;
use crate::code_mails::claimant;
use crate::db::Db;
use crate::mail::send_checkpoint_notice;
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::message_notifications::mailboxes;
use crate::order_mails::change_of;
use crate::sales_report::{Paid, ad_report};

/// The cursor identity of the subscription that reads the checkpoints.
pub const SUBSCRIPTION: &str = "timada-dashboard/checkpoints";
/// The cursor identity of the subscription that owes their notices.
pub const NOTICES_SUBSCRIPTION: &str = "timada-dashboard/checkpoint-notices";

/// How many days day 45 and day 120 look back over, and day 90 in weeks of
/// seven.
const D45_DAYS: u32 = 21;
const D90_DAYS: u32 = 28;
const D120_DAYS: u32 = 30;

/// How many of a site's pages day 90 looks at.
const D90_PAGES: i64 = 500;

/// The launches of one shop.
pub type Launches = timada_launch::Command<Executor>;

/// A checkpoint's name for a person.
pub fn checkpoint_label(checkpoint: u32) -> String {
    match checkpoint {
        120 => "Day 120 to 180".to_owned(),
        day => format!("Day {day}"),
    }
}

/// What a checkpoint reads, for a person.
pub fn checkpoint_reads(checkpoint: u32) -> &'static str {
    match checkpoint {
        21 => "The return on ad spend of the first campaign, as Google Ads counts it.",
        45 => "The return on ad spend from the shop's own orders, against its break-even.",
        90 => "Search Console: the categories in the first positions, and the impressions.",
        _ => "The margin once goods, costs and ads are paid, every channel together.",
    }
}

/// An advice or a decision, for a person.
pub fn advice_label(advice: &str) -> &'static str {
    match advice {
        timada_launch::GO_ON => "Go on",
        timada_launch::FIX => "Fix",
        timada_launch::STOP => "Stop or change niche",
        _ => "Not said",
    }
}

/// The day (`2026-11-20`) `days` after the moment `at`.
fn day_after(at: u64, days: u32) -> String {
    timada_core::civil::format_date(at + u64::from(days) * 86_400)
}

/// From `first` to `last` as the account's clocks count them.
fn period_of(account: &AdsAccount, first: &str, last: &str) -> Option<Period> {
    let zone = jiff::tz::TimeZone::get(&account.time_zone).unwrap_or(jiff::tz::TimeZone::UTC);
    let from = first.parse::<jiff::civil::Date>().ok()?;
    let to = last.parse::<jiff::civil::Date>().ok()?;
    Some(period_of_days(&zone, from, to))
}

/// What is left of the shop's sales from `first` to `last` once goods,
/// costs and Google Ads are paid: the amount in hundredths, and what it is
/// of the revenue in hundredths of a percent. `None` while nothing was
/// sold in the account's currency, or not every sale says what it left.
pub async fn after_ads(
    db: &Db,
    tenant_id: &str,
    account: &AdsAccount,
    (first, last): (&str, &str),
) -> anyhow::Result<Option<(i64, i64)>> {
    let Some(period) = period_of(account, first, last) else {
        return Ok(None);
    };
    let totals = timada_orders::sales_totals(db, tenant_id, period).await?;
    let Some(sold) = totals
        .iter()
        .find(|total| total.currency == account.currency)
        .filter(|total| total.orders > 0 && total.costed_orders == total.orders)
    else {
        return Ok(None);
    };
    let spend = ads_totals(db, tenant_id, &account.customer_id, first, last)
        .await?
        .figures()
        .cost_cents();
    let left = sold.profit_cents - spend;
    let net = sold.net_cents();
    Ok((net > 0).then(|| (left, (left as i128 * 10_000 / net as i128) as i64)))
}

/// Reads what the checkpoint of day `checkpoint` reads, for a campaign
/// that went live at `started`.
async fn read(
    store: &Executor,
    db: &Db,
    tenant_id: &str,
    checkpoint: u32,
    (campaign, account, started): (&CampaignState, &AdsAccount, u64),
    thresholds: &Thresholds,
) -> anyhow::Result<Reading> {
    let customer_id = account.customer_id.as_str();
    // The last whole day before the checkpoint's own.
    let last = day_after(started, checkpoint - 1);
    Ok(match checkpoint {
        21 => {
            let first = day_after(started, 0);
            let devices = ads_within_campaign(
                db,
                Within::Devices,
                tenant_id,
                (customer_id, &campaign.campaign_id),
                (&first, &last),
                10,
            )
            .await?;
            let sum = |of: fn(&timada_google::AdsLine) -> i64| -> u64 {
                devices.iter().map(of).sum::<i64>().max(0) as u64
            };
            Reading::Day21 {
                return_hundredths: return_of(
                    sum(|line| line.value_micros),
                    sum(|line| line.cost_micros),
                ),
                clicks: sum(|line| line.clicks),
            }
        }
        45 => {
            let first = day_before(&last, D45_DAYS - 1);
            let report = match period_of(account, &first, &last) {
                Some(period) => Some(ad_report(db, tenant_id, period, &first, &last).await?),
                None => None,
            };
            let ads = report
                .as_ref()
                .and_then(|report| report.channel(Paid::GoogleAds));
            // What the products those orders bought have to bring for 1
            // left: from the lines that say what they left.
            let (costed, profit) = report.iter().flat_map(|report| &report.products).fold(
                (0i64, 0i64),
                |(costed, profit), row| {
                    (costed + row.costed_sales_cents, profit + row.profit_cents)
                },
            );
            let tuned = timada_google::Command(store.clone())
                .load_tuning(tenant_id)
                .await?
                .and_then(|tuning| tuning.last_round)
                .is_some_and(|round| round.error.is_none() && round.at >= started);
            Reading::Day45 {
                return_hundredths: ads.and_then(|ads| ads.return_hundredths()),
                break_even: (costed > 0 && profit > 0)
                    .then(|| (costed as i128 * 100 / profit as i128) as i64),
                tuned,
            }
        }
        90 => {
            let site = timada_google::Command(store.clone())
                .load_connection(tenant_id)
                .await?
                .filter(|connection| connection.covers(ACCESS_SEARCH_CONSOLE))
                .and_then(|connection| connection.site);
            let held = match &site {
                Some(site) => last_search_day(db, tenant_id, site).await?,
                None => None,
            };
            let (Some(site), Some(last)) = (site, held) else {
                return Ok(Reading::Day90 {
                    read: false,
                    placed: 0,
                    impressions: 0,
                    impressions_before: 0,
                });
            };
            let first = day_before(&last, D90_DAYS - 1);
            let now = search_totals(db, tenant_id, &site, &first, &last).await?;
            let before = search_totals(
                db,
                tenant_id,
                &site,
                &day_before(&first, D90_DAYS),
                &day_before(&first, 1),
            )
            .await?;
            let others = timada_pages::Command::new(store.clone(), tenant_id)
                .languages()
                .await?
                .map(|languages| languages.others)
                .unwrap_or_default();
            let mut landings = Landings::new(db, tenant_id, &others);
            // Each category by what its pages did together, whatever the
            // language.
            let mut by_category: HashMap<String, (i64, i64)> = HashMap::new();
            for page in top_pages(db, tenant_id, &site, &first, &last, D90_PAGES).await? {
                if let Landing::Category { id, .. } = landings.of(&page.key).await? {
                    let sum = by_category.entry(id).or_default();
                    sum.0 += page.impressions;
                    sum.1 += page.weighted_position_milli;
                }
            }
            let limit = thresholds.d90_position as i64 * 1_000 + 500;
            let placed = by_category
                .values()
                .filter(|(impressions, weighted)| {
                    *impressions > 0 && weighted / impressions < limit
                })
                .count();
            Reading::Day90 {
                read: true,
                placed: placed as u64,
                impressions: now.impressions.max(0) as u64,
                impressions_before: before.impressions.max(0) as u64,
            }
        }
        _ => {
            let first = day_before(&last, D120_DAYS - 1);
            let now = after_ads(db, tenant_id, account, (&first, &last)).await?;
            let before = after_ads(
                db,
                tenant_id,
                account,
                (&day_before(&first, D120_DAYS), &day_before(&first, 1)),
            )
            .await?;
            Reading::Day120 {
                margin_bps: now.map(|(_, bps)| bps),
                margin_before_bps: before.map(|(_, bps)| bps),
            }
        }
    })
}

/// Reads and records the shop's checkpoint whose day came at `now`, when
/// it has one that was not read yet. Which one was recorded.
pub(crate) async fn read_due(
    store: &Executor,
    db: &Db,
    tenant_id: &str,
    now: u64,
) -> anyhow::Result<Option<u32>> {
    let Some(campaign) = held(store, tenant_id, CAMPAIGN_SHOPPING).await? else {
        return Ok(None);
    };
    let Some(started) = campaign.live_at else {
        return Ok(None);
    };
    let launches = Launches::new(store.clone(), tenant_id);
    let launch: Option<LaunchState> = launches.load_launch().await?;
    let Some(checkpoint) = checkpoint_due(launch.as_ref(), started, now) else {
        return Ok(None);
    };
    let Some((_, account)) = reach(store, tenant_id, &campaign.customer_id).await? else {
        return Ok(None);
    };
    let thresholds = launch
        .as_ref()
        .map(LaunchState::thresholds)
        .unwrap_or_default();
    let reading = read(
        store,
        db,
        tenant_id,
        checkpoint,
        (&campaign, &account, started),
        &thresholds,
    )
    .await?;
    let recorded = launches
        .record_checkpoint(LAUNCH_ACTOR, started, reading)
        .await?;
    if recorded {
        tracing::info!(tenant = %tenant_id, checkpoint, "a checkpoint was read");
    }
    Ok(recorded.then_some(checkpoint))
}

/// The store and the read models, to read checkpoints with.
#[derive(Clone)]
struct Parts {
    store: Executor,
    db: Db,
}

/// Reads a shop's checkpoint when the daily read of Google Ads ends for it
/// and the checkpoint's day has come. A saga: it follows the node's own
/// work group; the streams it listens to are the platform's.
pub fn subscription<E: evento::Executor>(store: Executor, db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Parts { store, db })
        .handler(on_ads_checked())
        .tenant("")
        .retry(5)
}

#[evento::subscription]
async fn on_ads_checked<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<AdsChecked>,
) -> anyhow::Result<()> {
    // Older days are still owed: the numbers are not whole yet.
    if event.data.behind {
        return Ok(());
    }
    let parts: Parts = context.extract();
    let tenant_id = event.data.tenant_id.as_str();
    // One a pass: the next comes with tomorrow's read at the latest.
    read_due(&parts.store, &parts.db, tenant_id, crate::auth::now()).await?;
    Ok(())
}

/// Follows every shop's checkpoints into the mailroom's table, and the
/// notices' own streams back into it. Every node runs it; the node that
/// claims a checkpoint's notice first is the one that mails.
pub fn notices_subscription<E: evento::Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(NOTICES_SUBSCRIPTION)
        .data(room)
        .handler(on_checkpoint_reached())
        .handler(on_notice_event())
        // A saga: it follows the node's own work group (the executor's
        // default routing key).
        .retry(5)
}

/// The page of the shop at `slug` that shows its checkpoints.
pub fn checkpoints_link(base_url: &str, slug: &str) -> String {
    format!("{}/t/{slug}/checkpoints", base_url.trim_end_matches('/'))
}

#[evento::subscription]
async fn on_checkpoint_reached<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<CheckpointReached>,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    let checkpoint = event.data.checkpoint;
    // The notice is on its way or went out already: the row follows where
    // it stands rather than owing it again.
    let change = match notice_projection()
        .load(notice_id(checkpoint))
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    {
        Some(notice) => change_of(&notice.sending),
        None => None,
    };
    let change = change.unwrap_or(Change::Owed {
        version: event.version,
        expires_at: event.timestamp + NOTICE_WINDOW_SECS,
    });
    room.record(
        Kind::CheckpointNotice,
        &event.tenant,
        &checkpoint.to_string(),
        change,
        crate::auth::now(),
    )
    .await
}

/// Follows a notice's own stream into the table: whichever of its events
/// this is, the row is written from where the stream stands now.
#[evento::subscription_all]
async fn on_notice_event<E: evento::Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Notice>,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    let Some(notice) = notice_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let Some(change) = change_of(&notice.sending) else {
        return Ok(());
    };
    room.record(
        Kind::CheckpointNotice,
        &event.tenant,
        &notice.checkpoint.to_string(),
        change,
        crate::auth::now(),
    )
    .await
}

/// Mails the notice of the checkpoint `owed` names at `now` (unix
/// seconds), from the mailroom: finds the owners, claims the notice, mails
/// each of them the numbers and what they say, records how it went.
pub(crate) async fn send(room: &Mailroom, owed: &OwedMail, now: u64) -> anyhow::Result<Outcome> {
    let tenant_id = owed.tenant_id.as_str();
    let Ok(checkpoint) = owed.stream_id.parse::<u32>() else {
        return Ok(Outcome::Dropped);
    };
    let Some(shop) = timada_tenancy::Command(room.store.clone())
        .load_tenant(tenant_id)
        .await?
    else {
        return Ok(Outcome::Dropped);
    };
    let launches = Launches::new(room.store.clone(), tenant_id);
    let reached = launches
        .load_launch()
        .await?
        .and_then(|launch| launch.reached(checkpoint).cloned());
    let Some(reached) = reached else {
        return Ok(Outcome::Dropped);
    };
    let owners: Vec<String> = shop
        .members
        .keys()
        .filter(|user_id| shop.is_owner(user_id))
        .cloned()
        .collect();
    let recipients = mailboxes(timada_identity::users_by_ids(&room.db, &owners).await?);
    if recipients.is_empty() {
        tracing::debug!(tenant = %tenant_id, "no owner to tell of a checkpoint");
        return Ok(Outcome::Dropped);
    }
    if !launches
        .claim_notice(
            checkpoint,
            owed.expires_at.max(0) as u64,
            claimant(room, now),
        )
        .await?
    {
        return Ok(Outcome::Left);
    }
    let cx = Cx::new().with_shared(room.mail.clone());
    let link = checkpoints_link(&room.base_url, &shop.slug);
    let label = checkpoint_label(checkpoint);
    // Each owner in turn; the first the server fails stops the round, the
    // retry mails everyone again. Addresses refused for good are skipped.
    let mut told = 0;
    let mut turned_down = None;
    for to in &recipients {
        let mail = send_checkpoint_notice(
            &cx,
            to,
            &shop.name,
            (&label, advice_label(&reached.advice), &reached.why),
            &reached.figures,
            &link,
        );
        match attempt(mail).await {
            Attempt::Sent => told += 1,
            Attempt::Undeliverable(reason) => {
                tracing::warn!(tenant = %tenant_id, %reason, "an owner's address refused the checkpoint notice");
                turned_down.get_or_insert(reason);
            }
            Attempt::NotSent(reason) => {
                tracing::warn!(tenant = %tenant_id, %reason, "checkpoint notice not sent; deferred");
                launches
                    .record_notice_not_sent(checkpoint, &room.node, &reason, now)
                    .await?;
                return Ok(Outcome::Deferred);
            }
        }
    }
    match turned_down {
        Some(reason) if told == 0 => {
            launches
                .record_notice_undeliverable(checkpoint, &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        _ => {
            tracing::info!(tenant = %tenant_id, checkpoint, owners = told, "checkpoint notice mailed");
            launches.record_notice_sent(checkpoint).await?;
            Ok(Outcome::Sent)
        }
    }
}

/// Where a shop's launch stands at `now`, for its home and its page: the
/// checkpoint ahead with its moment, the last one read, and what the last
/// thirty days left after ads against the target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Standing {
    /// When the first campaign went live; `None` before.
    pub started_at: Option<u64>,
    /// The checkpoint whose day has not come, and when it does.
    pub ahead: Option<(u32, u64)>,
    /// The last thirty days' profit after ads, in hundredths of the
    /// account's currency, when it is known.
    pub month_cents: Option<i64>,
    pub currency: String,
}

impl Standing {
    /// How far the last thirty days are toward `target_cents`, in
    /// hundredths; `None` while they are not known. Never under zero.
    pub fn progress_percent(&self, target_cents: u64) -> Option<i64> {
        let month = self.month_cents?;
        (target_cents > 0).then(|| (month.max(0) as i128 * 100 / target_cents as i128) as i64)
    }
}

/// Where the shop's launch stands at `now`.
pub async fn standing(
    store: &Executor,
    db: &Db,
    tenant_id: &str,
    now: u64,
) -> anyhow::Result<Standing> {
    let mut standing = Standing::default();
    let Some(campaign) = held(store, tenant_id, CAMPAIGN_SHOPPING).await? else {
        return Ok(standing);
    };
    standing.started_at = campaign.live_at;
    standing.ahead = campaign
        .live_at
        .and_then(|started| timada_launch::checkpoint_ahead(started, now));
    if let Some((_, account)) = reach(store, tenant_id, &campaign.customer_id).await? {
        let last = timada_core::civil::format_date(now.saturating_sub(86_400));
        let first = day_before(&last, D120_DAYS - 1);
        standing.month_cents = after_ads(db, tenant_id, &account, (&first, &last))
            .await?
            .map(|(left, _)| left);
        standing.currency = account.currency;
    }
    Ok(standing)
}

/// When the checkpoint of day `checkpoint` comes for a campaign live since
/// `started_at`.
pub fn due_at(started_at: u64, checkpoint: u32) -> u64 {
    checkpoint_at(started_at, checkpoint)
}

#[cfg(test)]
mod tests;
