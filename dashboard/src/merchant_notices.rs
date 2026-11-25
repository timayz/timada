//! Tells a shop's owners that Merchant Center turned against them: a
//! product refused that Google showed before, a new issue on the account.
//! A platform mail to each owner, with a link to the page that names what
//! changed, once a day at most.
//!
//! A read that says something turned for the worse
//! (`MerchantFeed::MerchantRead`) owes the notice of its day; this
//! subscription sees it on every node of the work group and hands it to
//! the node's [`Mailroom`], which sends off the cursor. The node that
//! writes the claim on the day's notice first is the one that mails
//! ([`timada_core::Sending`], `MerchantNotice`), one mail per owner, then
//! records how it went: a server that does not answer leaves
//! `MailDeferred`, and whichever node looks once the retry is due mails
//! again, until the retries run out or the day is over. A pass never fails
//! for a mail server.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_google::aggregator::{MerchantNotice, MerchantRead};
use timada_google::{
    Command, MERCHANT_NOTICE_WINDOW_SECS, merchant_notice_day, merchant_notice_id,
    merchant_notice_projection,
};
use topcoat::context::Cx;

use crate::code_mails::claimant;
use crate::mail::send_merchant_notice;
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::message_notifications::mailboxes;
use crate::order_mails::change_of;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/merchant-notices";

/// Follows what Merchant Center says of every shop into the mailroom's
/// table, and the notices' own streams back into it. Every node runs it;
/// the node that claims a day's notice first is the one that mails.
pub fn subscription<E: Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(room)
        .handler(on_merchant_read())
        .handler(on_notice_event())
        // The routing key stays the executor's: this node mails for the
        // reads of its own work group. The streams are the platform's.
        .tenant("")
        .retry(5)
}

/// The page of the shop at `slug` that says what Google holds against it.
pub fn merchant_link(base_url: &str, slug: &str) -> String {
    format!(
        "{}/t/{slug}/merchant-center",
        base_url.trim_end_matches('/')
    )
}

#[evento::subscription]
async fn on_merchant_read<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MerchantRead>,
) -> anyhow::Result<()> {
    // Nothing turned for the worse: nothing to tell.
    if event.data.newly_refused.is_empty() && event.data.new_account_issues.is_empty() {
        return Ok(());
    }
    let room: Arc<Mailroom> = context.extract();
    let tenant_id = event.data.tenant_id.as_str();
    let day = merchant_notice_day(event.timestamp);
    // The day's notice is on its way or went out already: the row follows
    // where it stands rather than owing it again.
    let change = match merchant_notice_projection()
        .load(merchant_notice_id(tenant_id, &day))
        .execute(context.executor)
        .await?
    {
        Some(notice) => change_of(&notice.sending),
        None => None,
    };
    let change = change.unwrap_or(Change::Owed {
        version: event.version,
        expires_at: event.timestamp + MERCHANT_NOTICE_WINDOW_SECS,
    });
    room.record(
        Kind::MerchantNotice,
        tenant_id,
        &day,
        change,
        crate::auth::now(),
    )
    .await
}

/// Follows a notice's own stream into the table: whichever of its events
/// this is, the row is written from where the stream stands now.
#[evento::subscription_all]
async fn on_notice_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<MerchantNotice>,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    let Some(notice) = merchant_notice_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let Some(change) = change_of(&notice.sending) else {
        return Ok(());
    };
    room.record(
        Kind::MerchantNotice,
        &notice.tenant_id,
        &notice.day,
        change,
        crate::auth::now(),
    )
    .await
}

/// Mails the notice of the day `owed` names at `now` (unix seconds), from
/// the mailroom: finds the owners, claims the notice, mails each of them
/// what the shop's feed says turned for the worse, records how it went.
pub(crate) async fn send(room: &Mailroom, owed: &OwedMail, now: u64) -> anyhow::Result<Outcome> {
    let tenant_id = owed.tenant_id.as_str();
    let day = owed.stream_id.as_str();
    let Some(shop) = timada_tenancy::Command(room.store.clone())
        .load_tenant(tenant_id)
        .await?
    else {
        return Ok(Outcome::Dropped);
    };
    let google = Command(room.store.clone());
    // What the mail counts: the last read's own word, on the account the
    // shop reads now.
    let account = google
        .load_connection(tenant_id)
        .await?
        .and_then(|connection| connection.merchant);
    let feed = match &account {
        Some(account) => {
            google
                .load_merchant_feed(tenant_id, &account.account_id)
                .await?
        }
        None => None,
    };
    let Some(feed) = feed else {
        return Ok(Outcome::Dropped);
    };
    let refused = feed
        .last
        .products
        .iter()
        .filter(|row| row.is_refused())
        .count();
    let issues = feed.last.account_issues.len();
    if refused == 0 && issues == 0 {
        // Put right before the mail left: nothing to tell any more.
        return Ok(Outcome::Dropped);
    }
    let owners: Vec<String> = shop
        .members
        .keys()
        .filter(|user_id| shop.is_owner(user_id))
        .cloned()
        .collect();
    let recipients = mailboxes(timada_identity::users_by_ids(&room.db, &owners).await?);
    if recipients.is_empty() {
        tracing::debug!(tenant = %tenant_id, "no owner to tell of what Merchant Center holds");
        return Ok(Outcome::Dropped);
    }
    if !google
        .claim_merchant_notice(
            tenant_id,
            day,
            owed.expires_at.max(0) as u64,
            claimant(room, now),
        )
        .await?
    {
        return Ok(Outcome::Left);
    }
    let cx = Cx::new().with_shared(room.mail.clone());
    let link = merchant_link(&room.base_url, &shop.slug);
    // Each owner in turn; the first the server fails stops the round, the
    // retry mails everyone again. Addresses refused for good are skipped.
    let mut told = 0;
    let mut turned_down = None;
    for to in &recipients {
        match attempt(send_merchant_notice(
            &cx, to, &shop.name, refused, issues, &link,
        ))
        .await
        {
            Attempt::Sent => told += 1,
            Attempt::Undeliverable(reason) => {
                tracing::warn!(tenant = %tenant_id, %reason, "an owner's address refused the Merchant Center notice");
                turned_down.get_or_insert(reason);
            }
            Attempt::NotSent(reason) => {
                tracing::warn!(tenant = %tenant_id, %reason, "Merchant Center notice not sent; deferred");
                google
                    .record_merchant_notice_not_sent(tenant_id, day, &room.node, &reason, now)
                    .await?;
                return Ok(Outcome::Deferred);
            }
        }
    }
    match turned_down {
        Some(reason) if told == 0 => {
            google
                .record_merchant_notice_undeliverable(tenant_id, day, &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        _ => {
            tracing::info!(tenant = %tenant_id, owners = told, "Merchant Center notice mailed");
            google.record_merchant_notice_sent(tenant_id, day).await?;
            Ok(Outcome::Sent)
        }
    }
}

#[cfg(test)]
mod tests;
