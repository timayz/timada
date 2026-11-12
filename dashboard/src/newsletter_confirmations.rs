//! Mails, from the shop, the link that confirms a request for its news: the
//! double opt-in of the newsletter.
//!
//! The command that records a request mails nothing: this subscription sees
//! it on every node and hands it to the node's [`Mailroom`], which sends off
//! the cursor, a lane per shop. The node that writes the claim on the
//! subscriber first is the one that mails ([`timada_core::Sending`]), with
//! the link derived again from the subscriber's key, then records how the
//! send went. A server that refuses the address for good leaves
//! `MailUndeliverable`, and the address may ask again at once; a server
//! that does not answer leaves `MailDeferred`, and whichever node looks
//! once the retry is due sends again, until the retries run out or the
//! link would be expired. A pass never fails for a mail server (#464).
//!
//! A link opens a page of the shop's storefront
//! (`{storefront}/newsletter/confirm/{token}`): a shop that has not said
//! where its storefront lives cannot have a link mailed, so such a request
//! is passed over (logged, left pending: the address may ask again once
//! the resend window is over). Both ways of recording a request refuse one
//! without a storefront address first, so this only happens to a request
//! recorded before the address was cleared.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_newsletter::CONFIRM_TTL_SECS;
use timada_newsletter::aggregator::{
    Confirmed, Forgotten, MailClaimed, MailDeferred, MailFailed, MailSent, MailUndeliverable,
    Requested, Unsubscribed,
};
use topcoat::context::Cx;

use crate::code_mails::claimant;
use crate::mail::send_newsletter_confirmation;
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::newsletter::confirm_link;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/newsletter-confirmations";

/// Follows every shop's subscribers into the mailroom's table. Every node
/// runs it; the node that claims a request's mail first is the one that
/// sends it.
pub fn subscription<E: Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(room)
        .handler(on_requested())
        .handler(on_confirmed())
        .handler(on_unsubscribed())
        .handler(on_forgotten())
        .handler(on_mail_claimed())
        .handler(on_mail_sent())
        .handler(on_mail_undeliverable())
        .handler(on_mail_deferred())
        .handler(on_mail_failed())
        // The routing key stays the executor's: this node mails the
        // confirmations of its own region.
        .retry(5)
}

/// Writes what an event of a subscriber says into the table.
async fn record<E: Executor>(
    context: &Context<'_, E>,
    tenant: &str,
    stream_id: &str,
    change: Change,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    room.record(
        Kind::NewsletterConfirmation,
        tenant,
        stream_id,
        change,
        crate::auth::now(),
    )
    .await
}

#[evento::subscription]
async fn on_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Requested>,
) -> anyhow::Result<()> {
    record(
        context,
        &event.tenant,
        &event.aggregate_id,
        Change::Owed {
            version: event.version,
            // The link lapses with the request.
            expires_at: event.timestamp + CONFIRM_TTL_SECS,
        },
    )
    .await
}

#[evento::subscription]
async fn on_confirmed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Confirmed>,
) -> anyhow::Result<()> {
    record(context, &event.tenant, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_unsubscribed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Unsubscribed>,
) -> anyhow::Result<()> {
    record(context, &event.tenant, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_forgotten<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Forgotten>,
) -> anyhow::Result<()> {
    record(context, &event.tenant, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_mail_claimed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailClaimed>,
) -> anyhow::Result<()> {
    record(
        context,
        &event.tenant,
        &event.aggregate_id,
        Change::Claimed {
            until: event.data.until,
        },
    )
    .await
}

#[evento::subscription]
async fn on_mail_sent<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailSent>,
) -> anyhow::Result<()> {
    record(context, &event.tenant, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_mail_undeliverable<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailUndeliverable>,
) -> anyhow::Result<()> {
    record(context, &event.tenant, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_mail_deferred<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailDeferred>,
) -> anyhow::Result<()> {
    record(
        context,
        &event.tenant,
        &event.aggregate_id,
        Change::Deferred {
            retry_at: event.data.retry_at,
            reason: event.data.reason.clone(),
        },
    )
    .await
}

#[evento::subscription]
async fn on_mail_failed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailFailed>,
) -> anyhow::Result<()> {
    record(
        context,
        &event.tenant,
        &event.aggregate_id,
        Change::Failed {
            reason: event.data.reason.clone(),
        },
    )
    .await
}

/// Sends the confirmation `owed` names at `now` (unix seconds), from the
/// mailroom: claims it, mails it, records how it went.
pub(crate) async fn send(room: &Mailroom, owed: &OwedMail, now: u64) -> anyhow::Result<Outcome> {
    let tenant = owed.tenant_id.as_str();
    let subscriber = owed.stream_id.as_str();
    let version = u16::try_from(owed.version).unwrap_or(u16::MAX);
    let Some(storefront) = timada_orders::Command::new(room.store.clone(), tenant)
        .storefront_address()
        .await?
    else {
        tracing::warn!(
            tenant,
            subscriber,
            "newsletter confirmation not mailed: the shop has no storefront address; the request is left pending"
        );
        return Ok(Outcome::Left);
    };
    let Some(shop) = timada_tenancy::Command(room.store.clone())
        .load_tenant(tenant)
        .await?
    else {
        return Ok(Outcome::Left);
    };
    let newsletter = timada_newsletter::Command::new(room.store.clone(), tenant);
    let Some(mail_to) = newsletter
        .claim_confirmation_mail(
            &*room.keys.for_tenant(tenant),
            subscriber,
            version,
            claimant(room, now),
        )
        .await?
    else {
        return Ok(Outcome::Left);
    };
    let cx = Cx::new(room.mail.clone());
    let sent = attempt(send_newsletter_confirmation(
        &cx,
        &shop,
        &mail_to.email,
        &confirm_link(&storefront, &mail_to.token),
        mail_to.language.as_deref(),
    ))
    .await;
    match sent {
        Attempt::Sent => {
            tracing::info!(tenant, subscriber, "newsletter confirmation mailed");
            newsletter
                .record_confirmation_mail_sent(subscriber, version)
                .await?;
            Ok(Outcome::Sent)
        }
        Attempt::Undeliverable(reason) => {
            tracing::warn!(tenant, subscriber, %reason, "newsletter confirmation not deliverable");
            newsletter
                .record_confirmation_mail_undeliverable(subscriber, version, &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        Attempt::NotSent(reason) => {
            tracing::warn!(tenant, subscriber, %reason, "newsletter confirmation not sent; deferred");
            newsletter
                .record_confirmation_mail_not_sent(subscriber, version, &room.node, &reason, now)
                .await?;
            Ok(Outcome::Deferred)
        }
    }
}

#[cfg(test)]
mod tests {
    use evento::{Evento, Turso};
    use timada_core::Sending;
    use timada_newsletter::{RequestOutcome, RequestSubscription, SOURCE_FORM};
    use topcoat::mail::{Mailbox, MemoryTransport};

    use super::*;
    use crate::code_mails::tests::{TENANT, mail_context, shop, vault};
    use crate::key_vault::KeyVault;
    use crate::mail_fault::fake_smtp;
    use crate::mailroom::tests::{room_with, rows, store};

    const STOREFRONT: &str = "https://shop.example";

    async fn with_storefront(store: &Turso) {
        timada_orders::Command::new(Evento::new(store.clone()), TENANT)
            .set_storefront_address("user-alice", Some(STOREFRONT))
            .await
            .unwrap();
    }

    async fn request(store: &Turso, vault: &Arc<KeyVault>, email: &str, now: u64) -> String {
        let outcome = timada_newsletter::Command::new(Evento::new(store.clone()), TENANT)
            .request_subscription(
                &*vault.for_tenant(TENANT),
                "storefront",
                RequestSubscription {
                    email: email.to_owned(),
                    language: Some("fr".to_owned()),
                    source: SOURCE_FORM.to_owned(),
                    time_zone: None,
                },
                now,
            )
            .await
            .unwrap();
        match outcome {
            RequestOutcome::ToConfirm { token, .. } => token,
            other => panic!("nothing to mail: {other:?}"),
        }
    }

    async fn pass(store: &Turso, room: &Arc<Mailroom>) {
        subscription::<Turso>(room.clone())
            .ephemeral()
            .no_retry()
            .run_once(store)
            .await
            .expect("a pass never fails for a mail");
    }

    async fn run(store: &Turso, room: &Arc<Mailroom>, now: u64) -> usize {
        pass(store, room).await;
        // The pass stamped the rows with the clock, which may have ticked
        // past the test's `now` meanwhile: sweep at whichever is later. A
        // failed send is stamped with that later time too, so a test reads
        // its retry off the stream rather than reckon it from `now`.
        room.sweep(now.max(crate::auth::now())).await.unwrap()
    }

    async fn sending(store: &Turso, email: &str) -> Sending {
        timada_newsletter::Command::new(Evento::new(store.clone()), TENANT)
            .subscriber_of(email)
            .await
            .unwrap()
            .unwrap()
            .mail
    }

    #[tokio::test]
    async fn a_confirmation_is_mailed_once_from_the_shop_with_the_link() {
        let store = store().await;
        let vault = vault();
        shop(&store).await;
        with_storefront(&store).await;
        let now = crate::auth::now();
        let token = request(&store, &vault, "Ada@Example.com", now).await;
        let transport = MemoryTransport::new();
        let mail = mail_context(transport.clone());
        let node_a = room_with(&store, mail.clone(), vault.clone(), "node-a");
        let node_b = room_with(&store, mail.clone(), vault.clone(), "node-b");

        assert_eq!(run(&store, &node_a, now).await, 1);
        assert_eq!(run(&store, &node_b, now).await, 0);
        assert_eq!(run(&store, &node_a, now).await, 0);

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to(), [Mailbox::new("ada@example.com").unwrap()]);
        assert_eq!(sent[0].from().unwrap().name(), Some("Acme Tea"));
        let cx = Cx::new(mail);
        // The HTML escapes "/" in the link and the wire folds long lines.
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap())
            .unwrap()
            .replace("=\r\n", "")
            .replace("&#x2f;", "/");
        assert!(wire.contains(&confirm_link(STOREFRONT, &token)), "{wire}");
        assert_eq!(sending(&store, "ada@example.com").await, Sending::Sent);
        assert!(rows(&store).await.is_empty());
    }

    #[tokio::test]
    async fn a_refused_address_is_recorded_and_a_shop_without_storefront_leaves_the_request_pending()
     {
        let store = store().await;
        let vault = vault();
        shop(&store).await;
        let now = crate::auth::now();
        request(&store, &vault, "ada@example.com", now).await;
        // No storefront address yet: nothing is claimed, nothing goes out,
        // and the mail stays owed for a later look.
        let transport = MemoryTransport::new();
        let node = room_with(
            &store,
            mail_context(transport.clone()),
            vault.clone(),
            "node-a",
        );
        assert_eq!(run(&store, &node, now).await, 0);
        assert!(transport.sent().is_empty());
        assert_eq!(sending(&store, "ada@example.com").await, Sending::Pending);
        let owed = rows(&store).await;
        assert_eq!(owed.len(), 1);
        assert!(owed[0].due_at > now as i64, "looked at again later");

        with_storefront(&store).await;
        let refusing = room_with(
            &store,
            mail_context(fake_smtp::transport(fake_smtp::serve(
                "550 5.1.1 no such user",
            ))),
            vault.clone(),
            "node-a",
        );
        assert_eq!(run(&store, &refusing, owed[0].due_at as u64).await, 0);
        assert!(sending(&store, "ada@example.com").await.is_undeliverable());
        // Refused for good: the address asks again at once, and the server
        // down defers the send.
        request(&store, &vault, "ada@example.com", now + 1).await;
        let down = room_with(
            &store,
            mail_context(fake_smtp::transport(fake_smtp::closed_port())),
            vault.clone(),
            "node-a",
        );
        assert_eq!(run(&store, &down, now + 1).await, 0);
        let mail = sending(&store, "ada@example.com").await;
        let Sending::Deferred {
            retry_at,
            attempts: 1,
            ..
        } = mail
        else {
            panic!("{mail:?}");
        };
        // The server back at the retry: sent.
        let transport = MemoryTransport::new();
        let back = room_with(&store, mail_context(transport.clone()), vault, "node-b");
        assert_eq!(run(&store, &back, retry_at).await, 1);
        assert_eq!(transport.sent().len(), 1);
        assert_eq!(sending(&store, "ada@example.com").await, Sending::Sent);
    }

    /// A request whose link would be expired is not mailed: the claim says
    /// so, and the table forgets it.
    #[tokio::test]
    async fn an_expired_request_is_not_mailed() {
        let store = store().await;
        let vault = vault();
        shop(&store).await;
        with_storefront(&store).await;
        let now = crate::auth::now();
        request(&store, &vault, "ada@example.com", now).await;
        let transport = MemoryTransport::new();
        let node = room_with(&store, mail_context(transport.clone()), vault, "node-a");
        pass(&store, &node).await;
        assert_eq!(rows(&store).await.len(), 1);
        assert_eq!(node.sweep(now + CONFIRM_TTL_SECS + 1).await.unwrap(), 0);
        assert!(transport.sent().is_empty());
        assert_eq!(sending(&store, "ada@example.com").await, Sending::Pending);
        assert!(rows(&store).await.is_empty());
    }
}
