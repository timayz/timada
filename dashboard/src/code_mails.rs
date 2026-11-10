//! Mails the one-time codes: the platform's, to prove an address when
//! signing up, signing in or resetting a password, and the shops', to their
//! shoppers under the shop's name.
//!
//! The command that issues a code mails nothing: it records the intent, and
//! these subscriptions see it on every node and hand it to the node's
//! [`Mailroom`], which sends off the cursor, a lane per shop. The node that
//! writes the claim on the challenge first is the one that mails
//! ([`timada_core::Sending`]); it derives the code from the nonce the event
//! carries, under the key the host holds, and records how the send went. A
//! server that refuses the address for good leaves `MailUndeliverable`, and
//! the person may ask again at once; a server that does not answer leaves
//! `MailDeferred`, and whichever node looks once the retry is due sends
//! again. A pass never fails for a mail server (#464).

use std::sync::Arc;

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::Claimant;
use topcoat::context::{Cx, app_context};

use crate::mail::{send_code, send_shopper_code};
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::seal::{EmailCodes, ShopperCodes};

/// The cursor identity of the subscription over the platform's codes, in
/// the store.
pub const PLATFORM_SUBSCRIPTION: &str = "timada-dashboard/platform-code-mails";
/// The cursor identity of the subscription over the shops' codes.
pub const SHOPPER_SUBSCRIPTION: &str = "timada-dashboard/shopper-code-mails";

/// Follows the platform's challenges into the mailroom's table. Every node
/// runs it; the node that claims a code's mail first is the one that sends
/// it.
pub fn platform_subscription<E: Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PLATFORM_SUBSCRIPTION)
        .data(room)
        .handler(on_email_code_issued())
        .handler(on_email_consumed())
        .handler(on_email_mail_claimed())
        .handler(on_email_mail_sent())
        .handler(on_email_mail_undeliverable())
        .handler(on_email_mail_deferred())
        .handler(on_email_mail_failed())
        // The routing key stays the executor's: this node mails the codes
        // of its own region.
        .tenant("")
        .retry(5)
}

/// Follows every shop's challenges, the same way.
pub fn shopper_subscription<E: Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SHOPPER_SUBSCRIPTION)
        .data(room)
        .handler(on_shopper_code_issued())
        .handler(on_shopper_code_consumed())
        .handler(on_shopper_mail_claimed())
        .handler(on_shopper_mail_sent())
        .handler(on_shopper_mail_undeliverable())
        .handler(on_shopper_mail_deferred())
        .handler(on_shopper_mail_failed())
        // The routing key stays the executor's: this node mails the codes
        // of its own region.
        .retry(5)
}

/// Writes what an event of a platform challenge says into the table.
async fn platform<E: Executor>(
    context: &Context<'_, E>,
    stream_id: &str,
    change: Change,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    room.record(
        Kind::PlatformCode,
        "",
        stream_id,
        change,
        crate::auth::now(),
    )
    .await
}

/// Writes what an event of a shop's challenge says into the table.
async fn shopper<E: Executor>(
    context: &Context<'_, E>,
    tenant: &str,
    stream_id: &str,
    change: Change,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    room.record(
        Kind::ShopperCode,
        tenant,
        stream_id,
        change,
        crate::auth::now(),
    )
    .await
}

#[evento::subscription]
async fn on_email_code_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_identity::aggregator::CodeIssued>,
) -> anyhow::Result<()> {
    platform(
        context,
        &event.aggregate_id,
        Change::Owed {
            version: event.version,
            expires_at: event.data.expires_at,
        },
    )
    .await
}

#[evento::subscription]
async fn on_email_consumed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_identity::aggregator::Consumed>,
) -> anyhow::Result<()> {
    platform(context, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_email_mail_claimed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_identity::aggregator::MailClaimed>,
) -> anyhow::Result<()> {
    platform(
        context,
        &event.aggregate_id,
        Change::Claimed {
            until: event.data.until,
        },
    )
    .await
}

#[evento::subscription]
async fn on_email_mail_sent<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_identity::aggregator::MailSent>,
) -> anyhow::Result<()> {
    platform(context, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_email_mail_undeliverable<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_identity::aggregator::MailUndeliverable>,
) -> anyhow::Result<()> {
    platform(context, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_email_mail_deferred<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_identity::aggregator::MailDeferred>,
) -> anyhow::Result<()> {
    platform(
        context,
        &event.aggregate_id,
        Change::Deferred {
            retry_at: event.data.retry_at,
            reason: event.data.reason.clone(),
        },
    )
    .await
}

#[evento::subscription]
async fn on_email_mail_failed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_identity::aggregator::MailFailed>,
) -> anyhow::Result<()> {
    platform(
        context,
        &event.aggregate_id,
        Change::Failed {
            reason: event.data.reason.clone(),
        },
    )
    .await
}

#[evento::subscription]
async fn on_shopper_code_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::CodeIssued>,
) -> anyhow::Result<()> {
    shopper(
        context,
        &event.tenant,
        &event.aggregate_id,
        Change::Owed {
            version: event.version,
            expires_at: event.data.expires_at,
        },
    )
    .await
}

#[evento::subscription]
async fn on_shopper_code_consumed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::CodeConsumed>,
) -> anyhow::Result<()> {
    shopper(context, &event.tenant, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_shopper_mail_claimed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::MailClaimed>,
) -> anyhow::Result<()> {
    shopper(
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
async fn on_shopper_mail_sent<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::MailSent>,
) -> anyhow::Result<()> {
    shopper(context, &event.tenant, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_shopper_mail_undeliverable<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::MailUndeliverable>,
) -> anyhow::Result<()> {
    shopper(context, &event.tenant, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_shopper_mail_deferred<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::MailDeferred>,
) -> anyhow::Result<()> {
    shopper(
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
async fn on_shopper_mail_failed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::MailFailed>,
) -> anyhow::Result<()> {
    shopper(
        context,
        &event.tenant,
        &event.aggregate_id,
        Change::Failed {
            reason: event.data.reason.clone(),
        },
    )
    .await
}

/// Sends the platform code `owed` names at `now` (unix seconds), from the
/// mailroom: claims it, mails it, records how it went.
pub(crate) async fn send_platform(
    room: &Mailroom,
    owed: &OwedMail,
    now: u64,
) -> anyhow::Result<Outcome> {
    let cx = Cx::new(room.mail.clone());
    let identity =
        timada_identity::Command::new(room.store.clone(), app_context::<EmailCodes>(&cx).0.clone());
    let challenge = owed.stream_id.as_str();
    let version = u16::try_from(owed.version).unwrap_or(u16::MAX);
    let Some(code) = identity
        .claim_code_mail(challenge, version, claimant(room, now))
        .await?
    else {
        return Ok(Outcome::Left);
    };
    match attempt(send_code(&cx, &code.email, code.purpose, &code.code)).await {
        Attempt::Sent => {
            tracing::info!(challenge, purpose = ?code.purpose, "sign-in code mailed");
            identity.record_code_mail_sent(challenge, version).await?;
            Ok(Outcome::Sent)
        }
        Attempt::Undeliverable(reason) => {
            tracing::warn!(challenge, %reason, "sign-in code not deliverable");
            identity
                .record_code_mail_undeliverable(challenge, version, &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        Attempt::NotSent(reason) => {
            tracing::warn!(challenge, %reason, "sign-in code not sent; deferred");
            identity
                .record_code_mail_not_sent(challenge, version, &room.node, &reason, now)
                .await?;
            Ok(Outcome::Deferred)
        }
    }
}

/// Sends the shopper code `owed` names at `now` (unix seconds), from the
/// mailroom, under the shop's name.
pub(crate) async fn send_shopper(
    room: &Mailroom,
    owed: &OwedMail,
    now: u64,
) -> anyhow::Result<Outcome> {
    let cx = Cx::new(room.mail.clone());
    // Without `secrets.key` no code could be issued through this node, and
    // none can be derived.
    let Some(codes) = app_context::<ShopperCodes>(&cx).0.clone() else {
        return Ok(Outcome::Left);
    };
    let tenant = owed.tenant_id.as_str();
    let Some(shop) = timada_tenancy::Command(room.store.clone())
        .load_tenant(tenant)
        .await?
    else {
        return Ok(Outcome::Left);
    };
    let shoppers = timada_shoppers::Command::new(room.store.clone(), tenant);
    let challenge = owed.stream_id.as_str();
    let version = u16::try_from(owed.version).unwrap_or(u16::MAX);
    let Some(code) = shoppers
        .claim_code_mail(
            &*room.keys.for_tenant(tenant),
            &*codes,
            challenge,
            version,
            claimant(room, now),
        )
        .await?
    else {
        return Ok(Outcome::Left);
    };
    let sent = attempt(send_shopper_code(
        &cx,
        &code.email,
        &shop,
        &code.code,
        code.language.as_deref(),
    ))
    .await;
    match sent {
        Attempt::Sent => {
            tracing::info!(tenant, challenge, "shopper sign-in code mailed");
            shoppers.record_code_mail_sent(challenge, version).await?;
            Ok(Outcome::Sent)
        }
        Attempt::Undeliverable(reason) => {
            tracing::warn!(tenant, challenge, %reason, "shopper sign-in code not deliverable");
            shoppers
                .record_code_mail_undeliverable(challenge, version, &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        Attempt::NotSent(reason) => {
            tracing::warn!(tenant, challenge, %reason, "shopper sign-in code not sent; deferred");
            shoppers
                .record_code_mail_not_sent(challenge, version, &room.node, &reason, now)
                .await?;
            Ok(Outcome::Deferred)
        }
    }
}

/// This node, asking at `now`.
pub(crate) fn claimant(room: &Mailroom, now: u64) -> Claimant<'_> {
    Claimant {
        node: &room.node,
        now,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use evento::{Evento, Turso};
    use timada_core::{RETRY_DELAYS_SECS, Sending};
    use timada_identity::ChallengePurpose;
    use topcoat::context::AppContext;
    use topcoat::mail::{MailConfig, Mailbox, MemoryTransport, Transport};

    use super::*;
    use crate::key_vault::KeyVault;
    use crate::mail::MailSender;
    use crate::mail_fault::fake_smtp;
    use crate::mailroom::tests::{room, room_with, rows, store};
    use crate::seal::{InvitationTokens, Sealer};
    use crate::shop_mail::ShopMail;

    pub(crate) const TENANT: &str = "tenant-1";

    /// What the sagas' mails need, over `transport`: the platform's sender,
    /// no shop server, and the keys the codes and links derive under.
    pub(crate) fn mail_context(transport: impl Transport + 'static) -> Arc<AppContext> {
        let mut app = AppContext::new();
        app.insert(MailConfig::builder().transport(transport).build());
        app.insert(MailSender("Timada <no-reply@example.com>".parse().unwrap()));
        app.insert(ShopMail::new(None, false));
        app.insert(email_codes());
        app.insert(ShopperCodes(Some(Arc::new(
            timada_core::HmacCodeVerifier::new([8; 32]),
        ))));
        app.insert(InvitationTokens(Arc::new(
            timada_core::HmacCodeVerifier::new([9; 32]),
        )));
        Arc::new(app)
    }

    pub(crate) fn email_codes() -> EmailCodes {
        EmailCodes(Arc::new(timada_core::HmacCodeVerifier::new([7; 32])))
    }

    fn shopper_codes() -> Arc<dyn timada_core::CodeVerifier> {
        Arc::new(timada_core::HmacCodeVerifier::new([8; 32]))
    }

    pub(crate) fn vault() -> Arc<KeyVault> {
        let dir = std::env::temp_dir().join(format!("timada-keys-{}", ulid::Ulid::generate()));
        Arc::new(KeyVault::new(
            &crate::config::DataKeysConfig { dir },
            Some(Sealer::new(
                "shoppers",
                "a-test-secret-of-thirty-two-chars!!",
            )),
            None,
        ))
    }

    /// The shop every tenant-scoped saga mails for, in the platform store.
    pub(crate) async fn shop(store: &Turso) -> timada_tenancy::TenantState {
        evento::append(TENANT)
            .original_version(0)
            .event(&timada_tenancy::aggregator::Created {
                slug: "acme".to_owned(),
                name: "Acme Tea".to_owned(),
                created_by: "user-alice".to_owned(),
            })
            .commit(store)
            .await
            .unwrap();
        timada_tenancy::Command(Evento::new(store.clone()))
            .load_tenant(TENANT)
            .await
            .unwrap()
            .unwrap()
    }

    fn identity(store: &Turso) -> timada_identity::Command<Evento> {
        timada_identity::Command::new(Evento::new(store.clone()), email_codes().0)
    }

    /// One pass of the platform subscription into `room`'s table: it never
    /// fails for a mail server.
    pub(crate) async fn platform_pass(store: &Turso, room: &Arc<Mailroom>) {
        platform_subscription::<Turso>(room.clone())
            .ephemeral()
            .no_retry()
            .run_once(store)
            .await
            .expect("a pass never fails for a mail");
    }

    pub(crate) async fn shopper_pass(store: &Turso, room: &Arc<Mailroom>) {
        shopper_subscription::<Turso>(room.clone())
            .ephemeral()
            .no_retry()
            .run_once(store)
            .await
            .expect("a pass never fails for a mail");
    }

    /// A pass and a sweep at `now`: what a node does with a new intent.
    async fn platform_run(store: &Turso, room: &Arc<Mailroom>, now: u64) -> usize {
        platform_pass(store, room).await;
        // The pass stamped the rows with the clock, which may have ticked
        // past the test's `now` meanwhile: sweep at whichever is later.
        room.sweep(now.max(crate::auth::now())).await.unwrap()
    }

    async fn shopper_run(store: &Turso, room: &Arc<Mailroom>, now: u64) -> usize {
        shopper_pass(store, room).await;
        // The pass stamped the rows with the clock, which may have ticked
        // past the test's `now` meanwhile: sweep at whichever is later.
        room.sweep(now.max(crate::auth::now())).await.unwrap()
    }

    #[tokio::test]
    async fn a_platform_code_is_mailed_once_however_many_nodes_see_it() {
        let store = store().await;
        let identity = identity(&store);
        let now = crate::auth::now();
        let issued = identity
            .issue_email_code("alice@example.com", ChallengePurpose::Register, now)
            .await
            .unwrap();
        let transport = MemoryTransport::new();
        let node_a = room(&store, transport.clone(), "node-a");
        let node_b = room(&store, transport.clone(), "node-b");

        assert_eq!(platform_run(&store, &node_a, now).await, 1);
        assert_eq!(platform_run(&store, &node_b, now).await, 0);
        // The event delivered again to the sender: nothing more goes out.
        assert_eq!(platform_run(&store, &node_a, now).await, 0);

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to(), [Mailbox::new("alice@example.com").unwrap()]);
        assert_eq!(sent[0].subject(), "Confirm your e-mail address for Timada");
        let cx = Cx::new(mail_context(MemoryTransport::new()));
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains(&issued.code), "{wire}");
        let challenge = identity
            .load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(challenge.mail, Sending::Sent);
        // Sent: the table owes nothing (both nodes' passes saw `MailSent`).
        assert!(rows(&store).await.is_empty());
    }

    #[tokio::test]
    async fn a_refused_address_is_recorded_and_the_person_may_try_again() {
        let store = store().await;
        let identity = identity(&store);
        let now = crate::auth::now();
        identity
            .issue_email_code("alice@example.com", ChallengePurpose::Register, now)
            .await
            .unwrap();
        let refusing = room(
            &store,
            fake_smtp::transport(fake_smtp::serve("550 5.1.1 no such user")),
            "node-a",
        );

        assert_eq!(platform_run(&store, &refusing, now).await, 0);

        let challenge = identity
            .load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap();
        match &challenge.mail {
            Sending::Undeliverable { reason } => {
                assert!(reason.contains("no such user"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        // The resend window is open: a corrected mailbox gets a code at once.
        identity
            .issue_email_code("alice@example.com", ChallengePurpose::Register, now + 1)
            .await
            .unwrap();
    }

    /// A server that cannot be reached, or answers that it is busy, defers
    /// the send: the pass is fine, the stream says when to try again, and
    /// nobody looks before then. Once the server is back, the next node to
    /// look sends.
    #[tokio::test]
    async fn a_server_that_cannot_be_reached_defers_the_send_and_never_fails_the_pass() {
        let store = store().await;
        let identity = identity(&store);
        let now = crate::auth::now();
        identity
            .issue_email_code("alice@example.com", ChallengePurpose::Register, now)
            .await
            .unwrap();
        let down = room(
            &store,
            fake_smtp::transport(fake_smtp::closed_port()),
            "node-a",
        );
        assert_eq!(platform_run(&store, &down, now).await, 0);
        let challenge = identity
            .load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap();
        let Sending::Deferred {
            retry_at,
            attempts,
            reason,
        } = &challenge.mail
        else {
            panic!("{:?}", challenge.mail);
        };
        assert_eq!((*retry_at, *attempts), (now + RETRY_DELAYS_SECS[0], 1));
        assert!(!reason.is_empty());
        // The table follows: due at the retry, and not looked at before.
        platform_pass(&store, &down).await;
        let owed = rows(&store).await;
        assert_eq!(owed.len(), 1);
        assert_eq!((owed[0].due_at, owed[0].attempts), (*retry_at as i64, 1));
        let back = room(&store, MemoryTransport::new(), "node-b");
        assert_eq!(back.sweep(retry_at - 1).await.unwrap(), 0);

        // A transient refusal at the retry defers again, for longer.
        let busy = room(
            &store,
            fake_smtp::transport(fake_smtp::serve("450 4.2.0 try again later")),
            "node-b",
        );
        assert_eq!(busy.sweep(*retry_at).await.unwrap(), 0);
        let challenge = identity
            .load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap();
        let Sending::Deferred {
            retry_at: again,
            attempts,
            ..
        } = &challenge.mail
        else {
            panic!("{:?}", challenge.mail);
        };
        assert_eq!((*again, *attempts), (retry_at + RETRY_DELAYS_SECS[1], 2));

        // The server back: whichever node looks once it is due sends.
        let transport = MemoryTransport::new();
        let back = room(&store, transport.clone(), "node-c");
        platform_pass(&store, &back).await;
        assert_eq!(back.sweep(*again).await.unwrap(), 1);
        assert_eq!(transport.sent().len(), 1);
        assert_eq!(
            identity
                .load_email_challenge("alice@example.com")
                .await
                .unwrap()
                .unwrap()
                .mail,
            Sending::Sent
        );
        platform_pass(&store, &back).await;
        assert!(rows(&store).await.is_empty());
    }

    /// A shop's code and the platform's land on two lanes of one sweep, and
    /// both go out.
    #[tokio::test]
    async fn codes_of_the_platform_and_of_a_shop_go_out_in_one_sweep() {
        let store = store().await;
        let vault = vault();
        shop(&store).await;
        let now = crate::auth::now();
        let shoppers = timada_shoppers::Command::new(Evento::new(store.clone()), TENANT);
        shoppers
            .issue_code(
                &*vault.for_tenant(TENANT),
                &*shopper_codes(),
                "ada@example.com",
                None,
                now,
            )
            .await
            .unwrap();
        identity(&store)
            .issue_email_code("alice@example.com", ChallengePurpose::Register, now)
            .await
            .unwrap();
        let transport = MemoryTransport::new();
        let node = room_with(&store, mail_context(transport.clone()), vault, "node-a");
        platform_pass(&store, &node).await;
        shopper_pass(&store, &node).await;
        assert_eq!(rows(&store).await.len(), 2);
        let sent = node.sweep(now.max(crate::auth::now())).await.unwrap();
        assert_eq!(sent, 2, "both lanes ran");
        assert_eq!(transport.sent().len(), 2);
    }

    #[tokio::test]
    async fn a_shopper_code_comes_from_the_shop_in_their_language_once() {
        let store = store().await;
        let vault = vault();
        shop(&store).await;
        let now = crate::auth::now();
        let shoppers = timada_shoppers::Command::new(Evento::new(store.clone()), TENANT);
        let issued = shoppers
            .issue_code(
                &*vault.for_tenant(TENANT),
                &*shopper_codes(),
                "Ada@Example.com",
                Some("fr-FR"),
                now,
            )
            .await
            .unwrap();
        let transport = MemoryTransport::new();
        let mail = mail_context(transport.clone());
        let node_a = room_with(&store, mail.clone(), vault.clone(), "node-a");
        let node_b = room_with(&store, mail.clone(), vault.clone(), "node-b");

        assert_eq!(shopper_run(&store, &node_a, now).await, 1);
        assert_eq!(shopper_run(&store, &node_b, now).await, 0);

        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to(), [Mailbox::new("ada@example.com").unwrap()]);
        assert_eq!(sent[0].subject(), "Your sign-in code for Acme Tea");
        assert_eq!(sent[0].from().unwrap().name(), Some("Acme Tea"));
        let cx = Cx::new(mail);
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains(&issued.code), "{wire}");
        assert_eq!(
            shoppers
                .load_challenge("ada@example.com")
                .await
                .unwrap()
                .unwrap()
                .mail,
            Sending::Sent
        );
    }

    #[tokio::test]
    async fn a_shoppers_refused_address_is_recorded_and_a_down_server_defers() {
        let store = store().await;
        let vault = vault();
        shop(&store).await;
        let now = crate::auth::now();
        let shoppers = timada_shoppers::Command::new(Evento::new(store.clone()), TENANT);
        shoppers
            .issue_code(
                &*vault.for_tenant(TENANT),
                &*shopper_codes(),
                "ada@example.com",
                None,
                now,
            )
            .await
            .unwrap();
        let refusing = room_with(
            &store,
            mail_context(fake_smtp::transport(fake_smtp::serve(
                "553 mailbox name not allowed",
            ))),
            vault.clone(),
            "node-a",
        );
        assert_eq!(shopper_run(&store, &refusing, now).await, 0);
        assert!(
            shoppers
                .load_challenge("ada@example.com")
                .await
                .unwrap()
                .unwrap()
                .mail
                .is_undeliverable()
        );
        // A later code, with the server down, is deferred instead.
        shoppers
            .issue_code(
                &*vault.for_tenant(TENANT),
                &*shopper_codes(),
                "ada@example.com",
                None,
                now + 1,
            )
            .await
            .unwrap();
        let down = room_with(
            &store,
            mail_context(fake_smtp::transport(fake_smtp::closed_port())),
            vault,
            "node-a",
        );
        assert_eq!(shopper_run(&store, &down, now + 1).await, 0);
        assert!(matches!(
            shoppers
                .load_challenge("ada@example.com")
                .await
                .unwrap()
                .unwrap()
                .mail,
            Sending::Deferred { attempts: 1, .. }
        ));
        // The table owes the deferred one alone.
        shopper_pass(&store, &down).await;
        let owed = rows(&store).await;
        assert_eq!(owed.len(), 1);
        assert_eq!(owed[0].attempts, 1);
    }
}
