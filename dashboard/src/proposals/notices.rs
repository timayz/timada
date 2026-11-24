//! Tells the members who may approve that proposals wait on the shop: a
//! platform mail to each, with a link to the list, once a day at most.
//!
//! `Prepared` owes the notice of its day; this subscription sees it on
//! every node and hands it to the node's [`Mailroom`], which sends off the
//! cursor. A proposal prepared by a member who may approve owes none: they
//! know. The node that writes the claim on the day's notice first is the
//! one that mails ([`timada_core::Sending`], `Notice`), one mail per
//! member, then records how it went: a server that does not answer leaves
//! `MailDeferred`, and whichever node looks once the retry is due mails
//! again, until the retries run out or the day is over. A pass never fails
//! for a mail server.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_proposals::aggregator::{Notice, Prepared};
use timada_proposals::{NOTICE_WINDOW_SECS, notice_day, notice_id, notice_projection};
use timada_tenancy::PERMISSION_PROPOSALS_APPROVE;
use topcoat::context::Cx;

use super::Proposals;
use crate::code_mails::claimant;
use crate::mail::send_proposals_notice;
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::message_notifications::{mailboxes, members_who_may};
use crate::order_mails::change_of;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/proposal-notices";

/// Follows every tenant's proposals into the mailroom's table, and the
/// notices' own streams back into it. Every node runs it; the node that
/// claims a day's notice first is the one that mails.
pub fn subscription<E: Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(room)
        .handler(on_prepared())
        .handler(on_notice_event())
        // A saga: it follows the node's own work group (the executor's
        // default routing key).
        .retry(5)
}

/// The list of the shop at `slug`, on the dashboard at `base_url`.
pub fn proposals_link(base_url: &str, slug: &str) -> String {
    format!("{}/t/{slug}/proposals", base_url.trim_end_matches('/'))
}

#[evento::subscription]
async fn on_prepared<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Prepared>,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    let Some(shop) = timada_tenancy::Command(room.store.clone())
        .load_tenant(&event.tenant)
        .await?
    else {
        return Ok(());
    };
    // Whoever may approve what they prepared needs no telling.
    if shop.can(&event.data.prepared_by, PERMISSION_PROPOSALS_APPROVE) {
        return Ok(());
    }
    let day = notice_day(event.timestamp);
    // The day's notice is on its way or went out already: the row follows
    // where it stands rather than owing it again.
    let change = match notice_projection()
        .load(notice_id(&day))
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
        Kind::ProposalNotice,
        &event.tenant,
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
        Kind::ProposalNotice,
        &event.tenant,
        &notice.day,
        change,
        crate::auth::now(),
    )
    .await
}

/// Mails the notice of the day `owed` names at `now` (unix seconds), from
/// the mailroom: finds the members to tell, claims the notice, mails each
/// of them, records how it went.
pub(crate) async fn send(room: &Mailroom, owed: &OwedMail, now: u64) -> anyhow::Result<Outcome> {
    let tenant_id = owed.tenant_id.as_str();
    let day = owed.stream_id.as_str();
    let Some(shop) = timada_tenancy::Command(room.store.clone())
        .load_tenant(tenant_id)
        .await?
    else {
        return Ok(Outcome::Dropped);
    };
    let recipients = mailboxes(
        timada_identity::users_by_ids(
            &room.db,
            &members_who_may(&shop, PERMISSION_PROPOSALS_APPROVE),
        )
        .await?,
    );
    if recipients.is_empty() {
        tracing::debug!(tenant = %tenant_id, "no member to tell of the proposals that wait");
        return Ok(Outcome::Dropped);
    }
    let proposals = Proposals::new(room.store.clone(), tenant_id);
    if !proposals
        .claim_notice(day, owed.expires_at.max(0) as u64, claimant(room, now))
        .await?
    {
        return Ok(Outcome::Left);
    }
    let cx = Cx::new().with_shared(room.mail.clone());
    let link = proposals_link(&room.base_url, &shop.slug);
    // Each member in turn; the first the server fails stops the round, the
    // retry mails everyone again. Addresses refused for good are skipped:
    // the notice went to whoever it could, unless that was nobody.
    let mut told = 0;
    let mut refused = None;
    for to in &recipients {
        match attempt(send_proposals_notice(&cx, to, &shop.name, &link)).await {
            Attempt::Sent => told += 1,
            Attempt::Undeliverable(reason) => {
                tracing::warn!(tenant = %tenant_id, %reason, "a member's address refused the proposals notice");
                refused.get_or_insert(reason);
            }
            Attempt::NotSent(reason) => {
                tracing::warn!(tenant = %tenant_id, %reason, "proposals notice not sent; deferred");
                proposals
                    .record_notice_not_sent(day, &room.node, &reason, now)
                    .await?;
                return Ok(Outcome::Deferred);
            }
        }
    }
    match refused {
        Some(reason) if told == 0 => {
            proposals.record_notice_undeliverable(day, &reason).await?;
            Ok(Outcome::Recorded)
        }
        _ => {
            tracing::info!(tenant = %tenant_id, members = told, "proposals notice mailed");
            proposals.record_notice_sent(day).await?;
            Ok(Outcome::Sent)
        }
    }
}

#[cfg(test)]
mod tests {
    use evento::{Evento, Turso};
    use timada_core::Sending;
    use timada_proposals::aggregator::Change as FieldChange;
    use timada_proposals::{KIND_PRODUCT, Prepare};
    use timada_tenancy::CreateTenant;
    use topcoat::mail::{Mailbox, MemoryTransport};

    use super::*;
    use crate::mailroom::tests::{BASE_URL, room, rows, store};

    /// A shop whose owner has an address, in the read models the notice
    /// reads; the shop's id.
    async fn shop(store: &Turso) -> String {
        let evento = Evento::new(store.clone());
        evento::append("user-alice")
            .original_version(0)
            .event(&timada_identity::aggregator::EmailRegistered {
                email: "alice@example.org".to_owned(),
                label: "Alice".to_owned(),
                password_hash: String::new(),
            })
            .commit(&evento)
            .await
            .unwrap();
        let shop = timada_tenancy::Command(evento.clone())
            .create_tenant(CreateTenant {
                name: "Acme Tea".to_owned(),
                owner_user_id: "user-alice".to_owned(),
            })
            .await
            .unwrap();
        timada_identity::read_model_subscription::<Turso>(crate::db::Db::from(store.clone()))
            .ephemeral()
            .run_once(store)
            .await
            .unwrap();
        shop.id
    }

    /// A proposal on `shop`, prepared by `actor`.
    async fn prepared(store: &Turso, shop: &str, actor: &str) -> String {
        timada_proposals::Command::new(Evento::new(store.clone()), shop)
            .prepare(
                actor,
                Prepare {
                    kind: KIND_PRODUCT.to_owned(),
                    subject_id: "p1".to_owned(),
                    subject_name: "Thé vert".to_owned(),
                    changes: vec![FieldChange {
                        field: "name".to_owned(),
                        before: "Thé vert".to_owned(),
                        after: "Thé vert sencha".to_owned(),
                    }],
                    reason: String::new(),
                    source: "writer".to_owned(),
                    valid_until: None,
                },
                crate::auth::now(),
            )
            .await
            .unwrap()
    }

    async fn pass(store: &Turso, room: &Arc<Mailroom>) {
        subscription::<Turso>(room.clone())
            .ephemeral()
            .no_retry()
            .run_once(store)
            .await
            .expect("a pass never fails for a mail");
    }

    async fn run(store: &Turso, room: &Arc<Mailroom>) -> usize {
        pass(store, room).await;
        room.sweep(crate::auth::now()).await.unwrap()
    }

    /// Two nodes see the proposals of a day; one tells the owner, once,
    /// however many arrive.
    #[tokio::test]
    async fn the_members_who_may_approve_are_told_once_a_day() {
        let store = store().await;
        let transport = MemoryTransport::new();
        let node_a = room(&store, transport.clone(), "node-a");
        let node_b = room(&store, transport.clone(), "node-b");
        let shop = shop(&store).await;
        prepared(&store, &shop, "timada-writer").await;
        prepared(&store, &shop, "timada-writer").await;

        assert_eq!(run(&store, &node_a).await, 1);
        assert_eq!(run(&store, &node_b).await, 0);

        let mails = transport.sent();
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].to(), [Mailbox::new("alice@example.org").unwrap()]);
        assert_eq!(
            mails[0].subject(),
            "Changes wait for your approval on Acme Tea"
        );
        let wire = String::from_utf8(mails[0].formatted())
            .unwrap()
            .replace("=\r\n", "");
        assert!(
            wire.contains(&proposals_link(BASE_URL, "acme-tea")),
            "{wire}"
        );
        // What the proposals would change stays on the dashboard.
        assert!(!wire.contains("sencha"), "{wire}");
        let day = notice_day(crate::auth::now());
        let notice = timada_proposals::Command::new(Evento::new(store.clone()), &shop)
            .load_notice(&day)
            .await
            .unwrap();
        // A day that turned between the proposal and here has its own
        // notice; either way the one that was owed went out.
        assert!(notice.is_none_or(|notice| notice.sending == Sending::Sent));
        pass(&store, &node_a).await;
        pass(&store, &node_b).await;
        assert!(rows(&store).await.is_empty());

        // One more the same day: the day was told already.
        prepared(&store, &shop, "timada-writer").await;
        assert_eq!(run(&store, &node_a).await, 0);
        assert_eq!(run(&store, &node_b).await, 0);
        assert_eq!(transport.sent().len(), 1);
        assert!(rows(&store).await.is_empty());
    }

    /// Whoever may approve what they prepared needs no telling.
    #[tokio::test]
    async fn a_proposal_its_preparer_may_approve_owes_no_notice() {
        let store = store().await;
        let transport = MemoryTransport::new();
        let node = room(&store, transport.clone(), "node-a");
        let shop = shop(&store).await;
        prepared(&store, &shop, "user-alice").await;
        assert_eq!(run(&store, &node).await, 0);
        assert!(transport.sent().is_empty());
        assert!(rows(&store).await.is_empty());
    }
}
