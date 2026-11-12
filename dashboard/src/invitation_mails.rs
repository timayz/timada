//! Mails the invitations to join a tenant: a platform mail with the link
//! whose token the stream's nonce derives under the host's key.
//!
//! The command that records an invitation mails nothing: this subscription
//! sees it on every node and hands it to the node's [`Mailroom`], which
//! sends off the cursor. The node that writes the claim on the invitation
//! first is the one that mails ([`timada_core::Sending`]), then records how
//! the send went. A server that refuses the address for good leaves
//! `MailUndeliverable`, which the members page shows, so the inviter sends
//! a corrected one; a server that does not answer leaves `MailDeferred`,
//! and whichever node looks once the retry is due sends again, until the
//! retries run out (`MailFailed`, shown too). A pass never fails for a mail
//! server (#464).

use std::sync::Arc;

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_tenancy::aggregator::{
    Accepted, MailClaimed, MailDeferred, MailFailed, MailSent, MailUndeliverable, Revoked, Sent,
};
use topcoat::context::{Cx, app_context};

use crate::app::t::slug::role_label;
use crate::code_mails::claimant;
use crate::mail::send_invitation;
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::seal::{EmailCodes, InvitationTokens};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/invitation-mails";

/// Follows the platform's invitations into the mailroom's table. Every node
/// runs it; the node that claims an invitation's mail first is the one that
/// sends it.
pub fn subscription<E: Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(room)
        .handler(on_sent())
        .handler(on_accepted())
        .handler(on_revoked())
        .handler(on_mail_claimed())
        .handler(on_mail_sent())
        .handler(on_mail_undeliverable())
        .handler(on_mail_deferred())
        .handler(on_mail_failed())
        // The routing key stays the executor's: this node mails the
        // invitations of its own region.
        .tenant("")
        .retry(5)
}

/// The page of the dashboard at `base_url` that `token` opens.
pub fn invitation_link(base_url: &str, token: &str) -> String {
    format!("{}/invitations/{token}", base_url.trim_end_matches('/'))
}

/// Writes what an event of an invitation says into the table.
async fn record<E: Executor>(
    context: &Context<'_, E>,
    stream_id: &str,
    change: Change,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    room.record(Kind::Invitation, "", stream_id, change, crate::auth::now())
        .await
}

#[evento::subscription]
async fn on_sent<E: Executor>(context: &Context<'_, E>, event: Event<Sent>) -> anyhow::Result<()> {
    record(
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
async fn on_accepted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Accepted>,
) -> anyhow::Result<()> {
    record(context, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_revoked<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Revoked>,
) -> anyhow::Result<()> {
    record(context, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_mail_claimed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailClaimed>,
) -> anyhow::Result<()> {
    record(
        context,
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
    record(context, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_mail_undeliverable<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailUndeliverable>,
) -> anyhow::Result<()> {
    record(context, &event.aggregate_id, Change::Settled).await
}

#[evento::subscription]
async fn on_mail_deferred<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailDeferred>,
) -> anyhow::Result<()> {
    record(
        context,
        &event.aggregate_id,
        Change::Deferred {
            retry_at: event.data.retry_at,
            attempts: event.data.attempts,
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
        &event.aggregate_id,
        Change::Failed {
            reason: event.data.reason.clone(),
            attempts: event.data.attempts,
        },
    )
    .await
}

/// Sends the invitation `owed` names at `now` (unix seconds), from the
/// mailroom: claims it, mails it, records how it went.
pub(crate) async fn send(room: &Mailroom, owed: &OwedMail, now: u64) -> anyhow::Result<Outcome> {
    let cx = Cx::new(room.mail.clone());
    let tenancy = timada_tenancy::Command(room.store.clone());
    let invitation = owed.stream_id.as_str();
    // The tenant and the inviter first, before anything is claimed: gone,
    // there is nothing to send.
    let Some(open) = tenancy.load_invitation(invitation).await? else {
        return Ok(Outcome::Left);
    };
    let Some(tenant) = tenancy.load_tenant(&open.tenant_id).await? else {
        return Ok(Outcome::Left);
    };
    let inviter =
        timada_identity::Command::new(room.store.clone(), app_context::<EmailCodes>(&cx).0.clone())
            .load_user(&open.invited_by)
            .await?
            .map_or_else(|| "A member".to_owned(), |user| user.label);
    let Some(mail) = tenancy
        .claim_invitation_mail(
            &*app_context::<InvitationTokens>(&cx).0,
            invitation,
            claimant(room, now),
        )
        .await?
    else {
        return Ok(Outcome::Left);
    };
    let link = invitation_link(&room.base_url, &mail.token);
    let role = role_label(&tenant, &mail.invitation.role);
    let sent = attempt(send_invitation(
        &cx,
        &mail.invitation.email,
        &tenant.name,
        &inviter,
        &role,
        &link,
    ))
    .await;
    match sent {
        Attempt::Sent => {
            tracing::info!(invitation, tenant = %tenant.id, "invitation mailed");
            tenancy.record_invitation_mail_sent(invitation).await?;
            Ok(Outcome::Sent)
        }
        Attempt::Undeliverable(reason) => {
            tracing::warn!(invitation, tenant = %tenant.id, %reason, "invitation not deliverable");
            tenancy
                .record_invitation_mail_undeliverable(invitation, &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        Attempt::NotSent(reason) => {
            tracing::warn!(invitation, tenant = %tenant.id, %reason, "invitation not sent; deferred");
            tenancy
                .record_invitation_mail_not_sent(invitation, &room.node, &reason, now)
                .await?;
            Ok(Outcome::Deferred)
        }
    }
}

#[cfg(test)]
mod tests {
    use evento::{Evento, Turso};
    use timada_core::{RETRY_DELAYS_SECS, Sending};
    use timada_tenancy::{InviteMember, ROLE_OWNER};
    use topcoat::mail::{Mailbox, MemoryTransport};

    use super::*;
    use crate::code_mails::tests::{TENANT, mail_context, shop};
    use crate::mail_fault::fake_smtp;
    use crate::mailroom::tests::{BASE_URL, room, rows, store};

    fn tokens() -> Arc<dyn timada_core::TokenDeriver> {
        Arc::new(timada_core::HmacCodeVerifier::new([9; 32]))
    }

    /// The owner of the shop, with an address, and the shop.
    async fn shop_with_owner(store: &Turso) {
        shop(store).await;
        evento::append(TENANT)
            .original_version(1)
            .event(&timada_tenancy::aggregator::MemberGranted {
                user_id: "user-alice".to_owned(),
                role: ROLE_OWNER.to_owned(),
            })
            .commit(store)
            .await
            .unwrap();
        evento::append("user-alice")
            .original_version(0)
            .event(&timada_identity::aggregator::Registered {
                account: "alice@example.com".to_owned(),
                label: "Alice".to_owned(),
                method: timada_identity::SignInMethod::OAuth2,
            })
            .commit(store)
            .await
            .unwrap();
    }

    async fn invite(store: &Turso, email: &str) -> timada_tenancy::SentInvitation {
        timada_tenancy::Command(Evento::new(store.clone()))
            .invite_member(
                &*tokens(),
                "user-alice",
                InviteMember {
                    tenant_id: TENANT.to_owned(),
                    email: email.to_owned(),
                    role: ROLE_OWNER.to_owned(),
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

    async fn run(store: &Turso, room: &Arc<Mailroom>, now: u64) -> usize {
        pass(store, room).await;
        // The pass stamped the rows with the clock, which may have ticked
        // past the test's `now` meanwhile: sweep at whichever is later. A
        // failed send is stamped with that later time too, so a test reads
        // its retry off the stream rather than reckon it from `now`.
        room.sweep(now.max(crate::auth::now())).await.unwrap()
    }

    async fn mail_of(store: &Turso, id: &str) -> Sending {
        timada_tenancy::Command(Evento::new(store.clone()))
            .load_invitation(id)
            .await
            .unwrap()
            .unwrap()
            .mail
    }

    #[tokio::test]
    async fn an_invitation_is_mailed_once_with_the_link_the_token_opens() {
        let store = store().await;
        shop_with_owner(&store).await;
        let sent = invite(&store, "Bob@Example.com").await;
        let now = crate::auth::now();
        let transport = MemoryTransport::new();
        let node_a = room(&store, transport.clone(), "node-a");
        let node_b = room(&store, transport.clone(), "node-b");

        assert_eq!(run(&store, &node_a, now).await, 1);
        assert_eq!(run(&store, &node_b, now).await, 0);
        assert_eq!(run(&store, &node_a, now).await, 0);

        let mails = transport.sent();
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].to(), [Mailbox::new("bob@example.com").unwrap()]);
        assert_eq!(
            mails[0].subject(),
            "You are invited to join Acme Tea on Timada"
        );
        let cx = Cx::new(mail_context(MemoryTransport::new()));
        // The wire folds long lines.
        let wire = String::from_utf8(mails[0].clone().formatted(&cx).unwrap())
            .unwrap()
            .replace("=\r\n", "");
        assert!(
            wire.contains(&invitation_link(BASE_URL, &sent.token)),
            "{wire}"
        );
        assert!(wire.contains("Alice"), "{wire}");
        assert!(wire.contains("owner"), "{wire}");
        let tenancy = timada_tenancy::Command(Evento::new(store.clone()));
        assert_eq!(mail_of(&store, &sent.invitation.id).await, Sending::Sent);
        // The link in the mail is the one the acceptance knows.
        assert!(
            tenancy
                .load_invitation_by_token(&sent.token)
                .await
                .unwrap()
                .is_some()
        );
        assert!(rows(&store).await.is_empty());
    }

    #[tokio::test]
    async fn a_refused_address_is_recorded_and_a_down_server_defers_the_send() {
        let store = store().await;
        shop_with_owner(&store).await;
        let now = crate::auth::now();
        let refused = invite(&store, "bob@example.com").await;
        let refusing = room(
            &store,
            fake_smtp::transport(fake_smtp::serve("550 5.1.1 no such user")),
            "node-a",
        );
        assert_eq!(run(&store, &refusing, now).await, 0);
        let mail = mail_of(&store, &refused.invitation.id).await;
        assert!(mail.is_undeliverable(), "{mail:?}");

        let pending = invite(&store, "carol@example.com").await;
        let down = room(
            &store,
            fake_smtp::transport(fake_smtp::closed_port()),
            "node-a",
        );
        assert_eq!(run(&store, &down, now).await, 0);
        let mail = mail_of(&store, &pending.invitation.id).await;
        let Sending::Deferred {
            retry_at,
            attempts: 1,
            ..
        } = mail
        else {
            panic!("{mail:?}");
        };
        // Withdrawn before the retry: nothing goes out, nothing is owed.
        timada_tenancy::Command(Evento::new(store.clone()))
            .revoke_invitation("user-alice", &pending.invitation.id, now)
            .await
            .unwrap();
        let transport = MemoryTransport::new();
        let back = room(&store, transport.clone(), "node-a");
        assert_eq!(run(&store, &back, retry_at).await, 0);
        assert!(transport.sent().is_empty());
        assert!(rows(&store).await.is_empty());
    }

    /// A server down for good: each retry of the schedule waits longer, and
    /// after the last the invitation is given up on, which the cluster page
    /// lists; a new invitation to the address starts afresh.
    #[tokio::test]
    async fn a_server_down_for_good_is_given_up_on_after_the_schedule() {
        let store = store().await;
        shop_with_owner(&store).await;
        let sent = invite(&store, "dan@example.com").await;
        let id = sent.invitation.id.as_str();
        let down = room(
            &store,
            fake_smtp::transport(fake_smtp::closed_port()),
            "node-a",
        );
        pass(&store, &down).await;
        // Read after the pass, the clock is no earlier than the row's due
        // time: the sweep takes it, and the first send fails at exactly
        // this time, which the schedule runs from.
        let mut now = crate::auth::now();
        assert_eq!(down.sweep(now).await.unwrap(), 0);
        for (tried, delay) in RETRY_DELAYS_SECS.iter().enumerate() {
            let Sending::Deferred {
                retry_at, attempts, ..
            } = mail_of(&store, id).await
            else {
                panic!("try {}: {:?}", tried + 1, mail_of(&store, id).await);
            };
            assert_eq!((retry_at, attempts), (now + delay, tried as u32 + 1));
            // Not before it is due.
            assert_eq!(run(&store, &down, retry_at - 1).await, 0);
            assert_eq!(mail_of(&store, id).await.attempts(), tried as u32 + 1);
            now = retry_at;
            assert_eq!(run(&store, &down, now).await, 0);
        }
        let mail = mail_of(&store, id).await;
        assert!(
            matches!(&mail, Sending::Failed { attempts, .. } if *attempts == RETRY_DELAYS_SECS.len() as u32 + 1),
            "{mail:?}"
        );
        pass(&store, &down).await;
        let owed = rows(&store).await;
        assert_eq!(owed.len(), 1);
        assert!(owed[0].failed);
        assert!(owed[0].reason.contains("refused") || !owed[0].reason.is_empty());
        let db = crate::db::Db::from(store.clone());
        assert_eq!(
            crate::mailroom::summary(&db).await.unwrap(),
            crate::mailroom::Summary { owed: 0, failed: 1 }
        );
        assert_eq!(
            crate::mailroom::failed_mails(&db, 10).await.unwrap()[0].stream_id,
            id
        );
        // The server back changes nothing for this one: it is settled.
        let transport = MemoryTransport::new();
        let back = room(&store, transport.clone(), "node-b");
        assert_eq!(run(&store, &back, now + 1).await, 0);
        assert!(transport.sent().is_empty());
    }
}
