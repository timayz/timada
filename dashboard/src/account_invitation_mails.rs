//! Mails root's invitations to create an account: a platform mail with the
//! link whose token the stream's nonce derives under the host's key.
//!
//! The command that records an invitation mails nothing: this subscription
//! sees it on every node and hands it to the node's [`Mailroom`], which
//! sends off the cursor. The node that writes the claim on the invitation
//! first is the one that mails ([`timada_core::Sending`]), then records how
//! the send went. A server that refuses the address for good leaves
//! `MailUndeliverable`, which the page of users shows, so root sends a
//! corrected one; a server that does not answer leaves `MailDeferred`, and
//! whichever node looks once the retry is due sends again, until the retries
//! run out (`MailFailed`, shown too). A pass never fails for a mail server
//! (#464).

use std::sync::Arc;

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_identity::invitation::{
    Accepted, MailClaimed, MailDeferred, MailFailed, MailSent, MailUndeliverable, Revoked, Sent,
};
use topcoat::context::{Cx, context};

use crate::code_mails::claimant;
use crate::mail::send_account_invitation;
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::seal::{EmailCodes, InvitationTokens};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/account-invitation-mails";

/// Follows the invitations to create an account into the mailroom's table.
/// Every node runs it; the node that claims an invitation's mail first is
/// the one that sends it.
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
        // invitations of its own work group.
        .tenant("")
        .retry(5)
}

/// The page of the dashboard at `base_url` that `token` opens.
pub fn invitation_link(base_url: &str, token: &str) -> String {
    format!("{}/auth/invitation/{token}", base_url.trim_end_matches('/'))
}

/// Writes what an event of an invitation says into the table.
async fn record<E: Executor>(
    context: &Context<'_, E>,
    stream_id: &str,
    change: Change,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    room.record(
        Kind::AccountInvitation,
        "",
        stream_id,
        change,
        crate::auth::now(),
    )
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
    let cx = Cx::new().with_shared(room.mail.clone());
    let identity =
        timada_identity::Command::new(room.store.clone(), context::<EmailCodes>(&cx).0.clone());
    let invitation = owed.stream_id.as_str();
    let Some(mail) = identity
        .claim_account_invitation_mail(
            &*context::<InvitationTokens>(&cx).0,
            invitation,
            claimant(room, now),
        )
        .await?
    else {
        return Ok(Outcome::Left);
    };
    let link = invitation_link(&room.base_url, &mail.token);
    let sent = attempt(send_account_invitation(&cx, &mail.invitation.email, &link)).await;
    match sent {
        Attempt::Sent => {
            tracing::info!(invitation, "invitation to create an account mailed");
            identity
                .record_account_invitation_mail_sent(invitation)
                .await?;
            Ok(Outcome::Sent)
        }
        Attempt::Undeliverable(reason) => {
            tracing::warn!(invitation, %reason, "invitation to create an account not deliverable");
            identity
                .record_account_invitation_mail_undeliverable(invitation, &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        Attempt::NotSent(reason) => {
            tracing::warn!(invitation, %reason, "invitation to create an account not sent; deferred");
            identity
                .record_account_invitation_mail_not_sent(invitation, &room.node, &reason, now)
                .await?;
            Ok(Outcome::Deferred)
        }
    }
}

#[cfg(test)]
mod tests {
    use evento::Turso;
    use timada_core::Sending;
    use timada_identity::{ROOT_USER_ID, RegisterInvited, SentAccountInvitation, SignInRoot};
    use topcoat::mail::{Mailbox, MemoryTransport};

    use super::*;
    use crate::mail_fault::fake_smtp;
    use crate::mailroom::tests::{BASE_URL, room, rows, store};

    /// The key the mailroom of the tests derives the links under.
    fn tokens(room: &Mailroom) -> Arc<dyn timada_core::TokenDeriver> {
        let cx = Cx::new().with_shared(room.mail.clone());
        context::<InvitationTokens>(&cx).0.clone()
    }

    fn identity(room: &Mailroom) -> timada_identity::Command<crate::cluster::Executor> {
        let cx = Cx::new().with_shared(room.mail.clone());
        timada_identity::Command::new(room.store.clone(), context::<EmailCodes>(&cx).0.clone())
    }

    const ROOT_EMAIL: &str = "root@example.com";
    /// The hash of "root test password".
    const ROOT_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$diXOV+PTSdOCfHj2wDAlMA$iVEeQUJsww/h81mHmrcmcJRWuZs2mY3bopNrchI/yog";

    /// Root signs in, which writes its account, then invites `email`.
    async fn invite(room: &Mailroom, email: &str) -> SentAccountInvitation {
        let identity = identity(room);
        let root = timada_identity::RootAccount::new(ROOT_EMAIL, ROOT_HASH).unwrap();
        identity
            .sign_in_root(
                &root,
                SignInRoot {
                    email: ROOT_EMAIL.to_owned(),
                    password: "root test password".to_owned(),
                    session_user_id: None,
                    context: Default::default(),
                },
            )
            .await
            .unwrap();
        identity
            .invite_account(&*tokens(room), ROOT_USER_ID, email, crate::auth::now())
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
        // past the test's `now` meanwhile: sweep at whichever is later.
        room.sweep(now.max(crate::auth::now())).await.unwrap()
    }

    async fn mail_of(room: &Mailroom, id: &str) -> Sending {
        identity(room)
            .load_account_invitation(id)
            .await
            .unwrap()
            .unwrap()
            .mail
    }

    #[tokio::test]
    async fn an_invitation_is_mailed_once_with_the_link_that_creates_the_account() {
        let store = store().await;
        let transport = MemoryTransport::new();
        let node_a = room(&store, transport.clone(), "node-a");
        let node_b = room(&store, transport.clone(), "node-b");
        let sent = invite(&node_a, "Bob@Example.com").await;
        let now = crate::auth::now();

        assert_eq!(run(&store, &node_a, now).await, 1);
        assert_eq!(run(&store, &node_b, now).await, 0);
        assert_eq!(run(&store, &node_a, now).await, 0);

        let mails = transport.sent();
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].to(), [Mailbox::new("bob@example.com").unwrap()]);
        assert_eq!(
            mails[0].subject(),
            "You are invited to create your Timada account"
        );
        // The wire folds long lines.
        let wire = String::from_utf8(mails[0].formatted())
            .unwrap()
            .replace("=\r\n", "");
        let link = invitation_link(BASE_URL, &sent.token);
        assert_eq!(
            link,
            format!("https://timada.example/auth/invitation/{}", sent.token)
        );
        assert!(wire.contains(&link), "{wire}");
        assert_eq!(mail_of(&node_a, &sent.invitation.id).await, Sending::Sent);
        assert!(rows(&store).await.is_empty());

        // The link in the mail is the one that creates the account.
        let done = identity(&node_a)
            .register_invited(
                RegisterInvited {
                    token: sent.token.clone(),
                    password_hash: ROOT_HASH.to_owned(),
                    context: Default::default(),
                },
                crate::auth::now(),
            )
            .await
            .unwrap();
        assert_eq!(done.label, "bob@example.com");
    }

    #[tokio::test]
    async fn a_refused_address_is_recorded_and_a_down_server_defers_the_send() {
        let store = store().await;
        let refusing = room(
            &store,
            fake_smtp::transport(fake_smtp::serve("550 5.1.1 no such user")),
            "node-a",
        );
        let now = crate::auth::now();
        let refused = invite(&refusing, "bob@example.com").await;
        assert_eq!(run(&store, &refusing, now).await, 0);
        let mail = mail_of(&refusing, &refused.invitation.id).await;
        assert!(mail.is_undeliverable(), "{mail:?}");

        let pending = invite(&refusing, "carol@example.com").await;
        let down = room(
            &store,
            fake_smtp::transport(fake_smtp::closed_port()),
            "node-a",
        );
        assert_eq!(run(&store, &down, now).await, 0);
        let mail = mail_of(&down, &pending.invitation.id).await;
        let Sending::Deferred {
            retry_at,
            attempts: 1,
            ..
        } = mail
        else {
            panic!("{mail:?}");
        };
        // Withdrawn before the retry: nothing goes out, nothing is owed.
        identity(&down)
            .revoke_account_invitation(ROOT_USER_ID, &pending.invitation.id, now)
            .await
            .unwrap();
        let transport = MemoryTransport::new();
        let back = room(&store, transport.clone(), "node-a");
        assert_eq!(run(&store, &back, retry_at).await, 0);
        assert!(transport.sent().is_empty());
        assert!(rows(&store).await.is_empty());
    }
}
