//! What Merchant Center says of a shop's products, where the daily read
//! stands, and the notice owed to the shop's owners when something turned
//! against them.
//!
//! A read is an event on the stream of the shop's feed
//! ([`merchant_feed_id`]), written only when something moved since the one
//! before: a product's standing, an issue, something held against the
//! account. The products live in the event, so a rebuilt read model and
//! every node of a cluster hold the same. The event also says what turned
//! for the worse since the read before (a product refused that was not, a
//! new issue on the account): that is decided here, against the stream,
//! and a subscription mails the owners from it.
//!
//! The read itself leaves one small event a pass on the shop's watch
//! ([`merchant_watch_id`]), which says when the next one is due.

use std::collections::{HashMap, HashSet};

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use timada_core::{Claim, Claimant, LEASE_SECS, MailOutcome, MailRecord, Sending};

use crate::aggregator::{
    MailClaimed, MailDeferred, MailFailed, MailSent, MailUndeliverable, MerchantAccountIssue,
    MerchantCheckFailed, MerchantChecked, MerchantFeed, MerchantIssue, MerchantNotice,
    MerchantProductRow, MerchantRead, MerchantWatch, Owed,
};
use crate::error::GoogleError;
use crate::value_object::{
    CHECK_EVERY_SECS, MAX_MERCHANT_ISSUES, MAX_MERCHANT_PRODUCT_ROWS, MAX_MERCHANT_TEXT_LEN,
    MAX_REASON_LEN, MERCHANT_ABSENT, MERCHANT_ACCEPTED, MERCHANT_ACCOUNT_SEVERITIES,
    MERCHANT_CONTEXTS, MERCHANT_PENDING, MERCHANT_REFUSED, MERCHANT_SEVERITIES, MERCHANT_STATUSES,
    merchant_feed_id, merchant_notice_id, merchant_watch_id, normalize_merchant_account,
    retry_delay_secs,
};

use super::Command;

/// How many times a write is made again when another node wrote the same
/// stream at the same moment.
const ATTEMPTS: usize = 5;

impl MerchantProductRow {
    /// Whether Google shows it nowhere it is sent, or not somewhere.
    pub fn is_refused(&self) -> bool {
        self.ads == MERCHANT_REFUSED || self.listings == MERCHANT_REFUSED
    }

    /// Whether Google accepts it everywhere it is sent, and it is sent
    /// somewhere.
    pub fn is_accepted(&self) -> bool {
        let fine = |status: &str| status == MERCHANT_ACCEPTED || status == MERCHANT_ABSENT;
        fine(&self.ads)
            && fine(&self.listings)
            && !(self.ads == MERCHANT_ABSENT && self.listings == MERCHANT_ABSENT)
    }

    /// What sorts the ones that need looking at first: refused (0), still
    /// looked at (1), shown with something held against it (2), fine (3).
    pub fn rank(&self) -> u8 {
        if self.is_refused() {
            0
        } else if self.ads == MERCHANT_PENDING || self.listings == MERCHANT_PENDING {
            1
        } else if !self.issues.is_empty() {
            2
        } else {
            3
        }
    }
}

/// What a read of Merchant Center gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MerchantReport {
    pub products: Vec<MerchantProductRow>,
    pub account_issues: Vec<MerchantAccountIssue>,
}

/// A text of Google's, short enough to keep.
fn text(value: &str) -> String {
    value.trim().chars().take(MAX_MERCHANT_TEXT_LEN).collect()
}

/// One of `known`, or `other`.
fn one_of(value: &str, known: &[&str], other: &str) -> String {
    if known.contains(&value) {
        value.to_owned()
    } else {
        other.to_owned()
    }
}

impl MerchantReport {
    /// The read as it is kept: each product once, the ones that need
    /// looking at first (then by id, so two nodes keep the same), the
    /// accepted ones cut past what a read keeps; how many were cut.
    fn kept(mut self) -> (Self, u32) {
        self.products.retain(|row| !row.item_id.trim().is_empty());
        for row in &mut self.products {
            row.item_id = row.item_id.trim().chars().take(200).collect();
            row.ads = one_of(&row.ads, &MERCHANT_STATUSES, MERCHANT_ABSENT);
            row.listings = one_of(&row.listings, &MERCHANT_STATUSES, MERCHANT_ABSENT);
            row.issues.truncate(MAX_MERCHANT_ISSUES);
            for issue in &mut row.issues {
                *issue = MerchantIssue {
                    code: text(&issue.code),
                    severity: one_of(&issue.severity, &MERCHANT_SEVERITIES, "none"),
                    attribute: text(&issue.attribute),
                    context: one_of(&issue.context, &MERCHANT_CONTEXTS, ""),
                    description: text(&issue.description),
                    detail: text(&issue.detail),
                    documentation: text(&issue.documentation),
                };
            }
        }
        self.products
            .sort_by(|a, b| (a.rank(), &a.item_id).cmp(&(b.rank(), &b.item_id)));
        self.products.dedup_by(|b, a| a.item_id == b.item_id);
        let more = self
            .products
            .len()
            .saturating_sub(MAX_MERCHANT_PRODUCT_ROWS) as u32;
        self.products.truncate(MAX_MERCHANT_PRODUCT_ROWS);

        self.account_issues
            .retain(|issue| !issue.title.trim().is_empty());
        self.account_issues.truncate(MAX_MERCHANT_ISSUES);
        for issue in &mut self.account_issues {
            *issue = MerchantAccountIssue {
                title: text(&issue.title),
                severity: one_of(&issue.severity, &MERCHANT_ACCOUNT_SEVERITIES, "suggestion"),
                detail: text(&issue.detail),
                documentation: text(&issue.documentation),
            };
        }
        self.account_issues.sort_by(|a, b| a.title.cmp(&b.title));
        self.account_issues.dedup_by(|b, a| a.title == b.title);
        (self, more)
    }
}

/// The write model of a shop's feed in Merchant Center: what the last read
/// gave.
#[evento::projection(id = id, name = "timada-google/MerchantFeedState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct MerchantFeedState {
    pub id: String,
    pub last: MerchantReport,
    /// How many accepted products the last read left out.
    pub more: u32,
    /// Unix seconds of the last read that moved something.
    pub read_at: u64,
}

pub fn merchant_feed_projection<E: Executor>() -> Projection<E, MerchantFeedState> {
    Projection::new::<MerchantFeed>()
        .handler(on_merchant_read())
        .strict()
}

#[evento::handler]
async fn on_merchant_read(
    event: Event<MerchantRead>,
    feed: &mut MerchantFeedState,
) -> anyhow::Result<()> {
    feed.id = event.aggregate_id.to_owned();
    feed.last = MerchantReport {
        products: event.data.products.to_owned(),
        account_issues: event.data.account_issues.to_owned(),
    };
    feed.more = event.data.more;
    feed.read_at = event.timestamp;
    Ok(())
}

/// What recording a read did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MerchantRecorded {
    /// Whether something moved since the read before, so it was written.
    pub written: bool,
    /// How many products are refused that were not, and how many issues
    /// the account has that it had not.
    pub newly_refused: usize,
    pub new_account_issues: usize,
}

/// The write model of a shop's watch: where the read of its account stands.
#[evento::projection(id = id, name = "timada-google/MerchantWatchState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct MerchantWatchState {
    pub id: String,
    /// The shop.
    pub tenant_id: String,
    /// The account last read.
    pub account_id: String,
    /// How many products the last pass that worked read.
    pub products: u32,
    /// Unix seconds of the last pass that worked; 0 before any did.
    pub checked_at: u64,
    /// How many passes failed in a row since.
    pub attempts: u32,
    /// Why the last one failed; `None` once one works.
    pub last_error: Option<String>,
    /// Unix seconds from which the next pass is due.
    pub next_at: u64,
}

pub fn merchant_watch_projection<E: Executor>() -> Projection<E, MerchantWatchState> {
    Projection::new::<MerchantWatch>()
        .handler(on_merchant_checked())
        .handler(on_merchant_check_failed())
        .strict()
}

#[evento::handler]
async fn on_merchant_checked(
    event: Event<MerchantChecked>,
    watch: &mut MerchantWatchState,
) -> anyhow::Result<()> {
    watch.id = event.aggregate_id.to_owned();
    watch.tenant_id = event.data.tenant_id.to_owned();
    watch.account_id = event.data.account_id.to_owned();
    watch.products = event.data.products;
    watch.checked_at = event.timestamp;
    watch.attempts = 0;
    watch.last_error = None;
    watch.next_at = event.timestamp + CHECK_EVERY_SECS;
    Ok(())
}

#[evento::handler]
async fn on_merchant_check_failed(
    event: Event<MerchantCheckFailed>,
    watch: &mut MerchantWatchState,
) -> anyhow::Result<()> {
    watch.id = event.aggregate_id.to_owned();
    watch.tenant_id = event.data.tenant_id.to_owned();
    // A first failure on another account starts from nothing.
    if watch.account_id != event.data.account_id {
        watch.checked_at = 0;
        watch.products = 0;
    }
    watch.account_id = event.data.account_id.to_owned();
    watch.attempts = event.data.attempts;
    watch.last_error = Some(event.data.reason.to_owned());
    watch.next_at = event.timestamp + retry_delay_secs(event.data.attempts);
    Ok(())
}

/// Where the notice of a shop's day stands. No stream is no claim yet.
#[evento::projection(id = id, name = "timada-google/MerchantNoticeState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct MerchantNoticeState {
    pub id: String,
    /// The shop.
    pub tenant_id: String,
    /// `2026-11-24`.
    pub day: String,
    /// Unix seconds: no retry is made past it.
    pub expires_at: u64,
    pub sending: Sending,
}

pub fn merchant_notice_projection<E: Executor>() -> Projection<E, MerchantNoticeState> {
    Projection::new::<MerchantNotice>()
        .handler(on_notice_owed())
        .handler(on_notice_claimed())
        .handler(on_notice_sent())
        .handler(on_notice_undeliverable())
        .handler(on_notice_deferred())
        .handler(on_notice_failed())
        .strict()
}

#[evento::handler]
async fn on_notice_owed(event: Event<Owed>, state: &mut MerchantNoticeState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.tenant_id = event.data.tenant_id.clone();
    state.day = event.data.day.clone();
    state.expires_at = event.data.expires_at;
    Ok(())
}

#[evento::handler]
async fn on_notice_claimed(
    event: Event<MailClaimed>,
    state: &mut MerchantNoticeState,
) -> anyhow::Result<()> {
    state.sending = state.sending.claimed(&event.data.by, event.data.until);
    Ok(())
}

#[evento::handler]
async fn on_notice_sent(
    event: Event<MailSent>,
    state: &mut MerchantNoticeState,
) -> anyhow::Result<()> {
    let _ = event;
    state.sending = Sending::Sent;
    Ok(())
}

#[evento::handler]
async fn on_notice_undeliverable(
    event: Event<MailUndeliverable>,
    state: &mut MerchantNoticeState,
) -> anyhow::Result<()> {
    state.sending = Sending::Undeliverable {
        reason: event.data.reason.clone(),
    };
    Ok(())
}

#[evento::handler]
async fn on_notice_deferred(
    event: Event<MailDeferred>,
    state: &mut MerchantNoticeState,
) -> anyhow::Result<()> {
    state.sending = Sending::Deferred {
        retry_at: event.data.retry_at,
        attempts: event.data.attempts,
        reason: event.data.reason.clone(),
    };
    Ok(())
}

#[evento::handler]
async fn on_notice_failed(
    event: Event<MailFailed>,
    state: &mut MerchantNoticeState,
) -> anyhow::Result<()> {
    state.sending = Sending::Failed {
        reason: event.data.reason.clone(),
        attempts: event.data.attempts,
    };
    Ok(())
}

impl<E: Executor> Command<E> {
    /// What Merchant Center said of the shop's products at the last read
    /// that moved something; `None` before the first.
    pub async fn load_merchant_feed(
        &self,
        tenant_id: &str,
        account_id: &str,
    ) -> anyhow::Result<Option<MerchantFeedState>> {
        merchant_feed_projection()
            .load(merchant_feed_id(tenant_id, account_id))
            .execute(&self.0)
            .await
    }

    /// Where the read of a shop's Merchant Center account stands; `None`
    /// before its first pass.
    pub async fn load_merchant_watch(
        &self,
        tenant_id: &str,
    ) -> anyhow::Result<Option<MerchantWatchState>> {
        merchant_watch_projection()
            .load(merchant_watch_id(tenant_id))
            .execute(&self.0)
            .await
    }

    /// Where the notice of the shop's `day` stands, once a node claimed it.
    pub async fn load_merchant_notice(
        &self,
        tenant_id: &str,
        day: &str,
    ) -> anyhow::Result<Option<MerchantNoticeState>> {
        merchant_notice_projection()
            .load(merchant_notice_id(tenant_id, day))
            .execute(&self.0)
            .await
    }

    /// Writes one event to the shop's watch, made from the watch as it
    /// stands; again when another node wrote it meanwhile.
    async fn write_merchant_watch<D, F>(
        &self,
        tenant_id: &str,
        routing_key: Option<String>,
        event: F,
    ) -> Result<(), GoogleError>
    where
        D: evento::AggregateEvent + bitcode::Encode,
        F: Fn(Option<&MerchantWatchState>) -> D,
    {
        for _ in 0..ATTEMPTS {
            let loaded = self.load_merchant_watch(tenant_id).await?;
            let mut write = match &loaded {
                Some(watch) => watch.write()?,
                None => {
                    let mut write = evento::append(merchant_watch_id(tenant_id));
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

    async fn record_merchant_notice(
        &self,
        tenant_id: &str,
        day: &str,
        outcome: MailOutcome<'_>,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        for _ in 0..ATTEMPTS {
            // No claim, nothing to record over.
            let Some(state) = self.load_merchant_notice(tenant_id, day).await? else {
                return Ok(());
            };
            let record = state.sending.record(outcome, state.expires_at);
            if record == MailRecord::Nothing {
                return Ok(());
            }
            let mut write = state.write()?;
            write.routing_key_opt(routing_key.clone());
            match record {
                MailRecord::Sent => write.event(&MailSent),
                MailRecord::Undeliverable(reason) => write.event(&MailUndeliverable { reason }),
                MailRecord::Deferred {
                    retry_at,
                    attempts,
                    reason,
                } => write.event(&MailDeferred {
                    retry_at,
                    attempts,
                    reason,
                }),
                MailRecord::Failed { reason, attempts } => {
                    write.event(&MailFailed { reason, attempts })
                }
                MailRecord::Nothing => unreachable!("returned above"),
            };
            match write.commit(&self.0).await {
                Ok(_) => return Ok(()),
                // Another node wrote meanwhile: decide again on its word.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Records what Merchant Center says of the shop's products, when it
    /// moved since the read before, and says what turned for the worse:
    /// the products refused that were not, the issues the account did not
    /// have. A first read turns nothing: there is no before.
    pub async fn record_merchant_read(
        &self,
        actor: &str,
        tenant_id: &str,
        account_id: &str,
        report: MerchantReport,
        routing_key: Option<String>,
    ) -> Result<MerchantRecorded, GoogleError> {
        let account_id = normalize_merchant_account(account_id)?;
        let (report, more) = report.kept();
        let id = merchant_feed_id(tenant_id, &account_id);
        for _ in 0..ATTEMPTS {
            let loaded = merchant_feed_projection()
                .load(&id)
                .execute(&self.0)
                .await?;
            let (mut newly_refused, mut new_account_issues) = (Vec::new(), Vec::new());
            let mut write = match &loaded {
                Some(feed) if feed.last == report && feed.more == more => {
                    return Ok(MerchantRecorded::default());
                }
                Some(feed) => {
                    let before: HashMap<&str, bool> = feed
                        .last
                        .products
                        .iter()
                        .map(|row| (row.item_id.as_str(), row.is_refused()))
                        .collect();
                    newly_refused = report
                        .products
                        .iter()
                        .filter(|row| row.is_refused())
                        .filter(|row| match before.get(row.item_id.as_str()) {
                            Some(refused) => !refused,
                            // One of the accepted ones the read before
                            // left out.
                            None => feed.more > 0,
                        })
                        .map(|row| row.item_id.clone())
                        .collect();
                    let had: HashSet<&str> = feed
                        .last
                        .account_issues
                        .iter()
                        .map(|issue| issue.title.as_str())
                        .collect();
                    new_account_issues = report
                        .account_issues
                        .iter()
                        .filter(|issue| !had.contains(issue.title.as_str()))
                        .map(|issue| issue.title.clone())
                        .collect();
                    feed.write()?
                }
                None => {
                    let mut write = evento::append(&id);
                    write.original_version(0);
                    write
                }
            };
            let recorded = MerchantRecorded {
                written: true,
                newly_refused: newly_refused.len(),
                new_account_issues: new_account_issues.len(),
            };
            write
                .routing_key_opt(routing_key.clone())
                .event(&MerchantRead {
                    tenant_id: tenant_id.to_owned(),
                    account_id: account_id.clone(),
                    products: report.products.clone(),
                    more,
                    account_issues: report.account_issues.clone(),
                    newly_refused,
                    new_account_issues,
                    read_by: actor.to_owned(),
                });
            match write.commit(&self.0).await {
                Ok(_) => return Ok(recorded),
                // Another node read at the same moment: decided again
                // against what it wrote.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// Records that a pass over the shop's account read its `products`
    /// products: the next one is due a day later.
    pub async fn record_merchant_check(
        &self,
        actor: &str,
        tenant_id: &str,
        account_id: &str,
        products: u32,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let account_id = normalize_merchant_account(account_id)?;
        self.write_merchant_watch(tenant_id, routing_key, |_| MerchantChecked {
            tenant_id: tenant_id.to_owned(),
            account_id: account_id.clone(),
            products,
            checked_by: actor.to_owned(),
        })
        .await
    }

    /// Records that a pass over the shop's account failed, and why: the
    /// next one waits longer after each failure in a row.
    pub async fn record_merchant_failure(
        &self,
        actor: &str,
        tenant_id: &str,
        account_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        let account_id = normalize_merchant_account(account_id)?;
        let reason: String = reason.trim().chars().take(MAX_REASON_LEN).collect();
        self.write_merchant_watch(tenant_id, routing_key, |watch| MerchantCheckFailed {
            tenant_id: tenant_id.to_owned(),
            account_id: account_id.clone(),
            reason: reason.clone(),
            attempts: watch
                .filter(|watch| watch.account_id == account_id)
                .map_or(0, |watch| watch.attempts)
                .saturating_add(1),
            failed_by: actor.to_owned(),
        })
        .await
    }

    /// Takes, for `claimant`, the telling of what turned against the shop
    /// on `day`, worth sending until `expires_at` (unix seconds). `true`
    /// when this node mails: it took the claim, or holds it already;
    /// `false` when another node is sending, the next try is not due yet,
    /// or the outcome is recorded. The first claim writes what the stream
    /// is about with it.
    pub async fn claim_merchant_notice(
        &self,
        tenant_id: &str,
        day: &str,
        expires_at: u64,
        claimant: Claimant<'_>,
        routing_key: Option<String>,
    ) -> Result<bool, GoogleError> {
        let Claimant { node, now } = claimant;
        for _ in 0..ATTEMPTS {
            let state = self.load_merchant_notice(tenant_id, day).await?;
            let sending = state.as_ref().map(|state| &state.sending);
            match sending.map_or(Claim::Take, |sending| sending.claim(node, now)) {
                Claim::Leave => return Ok(false),
                Claim::Held => return Ok(true),
                Claim::Take => {}
            }
            let mut write = match &state {
                Some(state) => state.write()?,
                None => {
                    let mut write = evento::append(merchant_notice_id(tenant_id, day));
                    write.original_version(0).event(&Owed {
                        tenant_id: tenant_id.to_owned(),
                        day: day.to_owned(),
                        expires_at,
                    });
                    write
                }
            };
            write
                .routing_key_opt(routing_key.clone())
                .event(&MailClaimed {
                    by: node.to_owned(),
                    until: now + LEASE_SECS,
                });
            match write.commit(&self.0).await {
                Ok(_) => return Ok(true),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// Records that the notice of the shop's `day` was handed to the mail
    /// server. Nothing is written once the outcome is recorded already.
    pub async fn record_merchant_notice_sent(
        &self,
        tenant_id: &str,
        day: &str,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        self.record_merchant_notice(tenant_id, day, MailOutcome::Sent, routing_key)
            .await
    }

    /// Records that the mail server refused every owner's address for
    /// good, with what it said of the first.
    pub async fn record_merchant_notice_undeliverable(
        &self,
        tenant_id: &str,
        day: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        self.record_merchant_notice(
            tenant_id,
            day,
            MailOutcome::Undeliverable(reason),
            routing_key,
        )
        .await
    }

    /// Records that `node`'s send failed at `now` (unix seconds) for a
    /// reason worth a retry: deferred to the next try, or given up on once
    /// none is left before the notice's window closes.
    pub async fn record_merchant_notice_not_sent(
        &self,
        tenant_id: &str,
        day: &str,
        node: &str,
        reason: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), GoogleError> {
        self.record_merchant_notice(
            tenant_id,
            day,
            MailOutcome::NotSent { node, reason, now },
            routing_key,
        )
        .await
    }
}
