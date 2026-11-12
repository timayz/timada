//! Tells the shop's members a contact message arrived: a platform mail to
//! every member whose role lets them read the messages, with a link to the
//! inbox. The mail carries the visitor's first name and nothing else they
//! wrote, so what the shop may later erase does not live on in mailboxes.
//!
//! `Received` owes the notice; this subscription sees it on every node and
//! hands it to the node's [`Mailroom`], which sends off the cursor (#482).
//! The node that writes the claim on the notice's stream first is the one
//! that mails ([`timada_core::Sending`], `MessageNotification`), one mail per
//! member, then records how it went: a server that does not answer leaves
//! `MailDeferred`, and whichever node looks once the retry is due mails
//! again, every member included (a member told twice is the price of the
//! others being told at all), until the retries run out or the notice's day
//! is over (`MailFailed`, which `/admin/cluster` shows). A pass never fails
//! for a mail server.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_identity::UserRow;
use timada_support::MESSAGE_NOTICE_WINDOW_SECS;
use timada_support::aggregator::{MessageNotification, Received};
use timada_tenancy::{PERMISSION_SUPPORT_READ, TenantState};
use topcoat::context::Cx;

use crate::code_mails::claimant;
use crate::mail::send_message_notification;
use crate::mail_fault::{Attempt, attempt};
use crate::mailroom::{Change, Kind, Mailroom, Outcome, OwedMail};
use crate::order_mails::change_of;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/message-notifications";

/// Follows every tenant's messages into the mailroom's table, and the
/// notices' own streams back into it. Every node runs it; the node that
/// claims a message's notice first is the one that mails.
pub fn subscription<E: Executor>(room: Arc<Mailroom>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(room)
        .handler(on_received())
        .handler(on_notification_event())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

/// The inbox of the shop at `slug`, on the dashboard at `base_url`.
pub fn inbox_link(base_url: &str, slug: &str) -> String {
    format!("{}/t/{slug}/messages", base_url.trim_end_matches('/'))
}

#[evento::subscription]
async fn on_received<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Received>,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    room.record(
        Kind::MessageNotice,
        &event.tenant,
        &event.aggregate_id,
        Change::Owed {
            version: event.version,
            expires_at: event.timestamp + MESSAGE_NOTICE_WINDOW_SECS,
        },
        crate::auth::now(),
    )
    .await
}

/// Follows a notice's own stream into the table: where its sending stands
/// after this event is what the stream says now, which names the message
/// the row is keyed by.
#[evento::subscription_all]
async fn on_notification_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<MessageNotification>,
) -> anyhow::Result<()> {
    let room: Arc<Mailroom> = context.extract();
    let Some(notice) = timada_support::message_notification_projection()
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
        Kind::MessageNotice,
        &event.tenant,
        &notice.message_id,
        change,
        crate::auth::now(),
    )
    .await
}

/// Mails the notice of the message `owed` names at `now` (unix seconds),
/// from the mailroom: opens the message, finds the members to tell, claims
/// the notice, mails each of them, records how it went.
pub(crate) async fn send(room: &Mailroom, owed: &OwedMail, now: u64) -> anyhow::Result<Outcome> {
    let tenant_id = owed.tenant_id.as_str();
    let message_id = owed.stream_id.as_str();
    let support = timada_support::Command::new(room.store.clone(), tenant_id);
    // Erased meanwhile: nothing to tell.
    let Some(state) = support.load_message(message_id).await? else {
        return Ok(Outcome::Dropped);
    };
    // A key not here yet is the store's to bring: the lane stops for now.
    let Some(message) = state.open_with(&*room.keys.for_tenant(tenant_id)).await? else {
        return Ok(Outcome::Dropped);
    };
    let Some(shop) = timada_tenancy::Command(room.store.clone())
        .load_tenant(tenant_id)
        .await?
    else {
        return Ok(Outcome::Left);
    };
    let recipients = mailboxes(
        timada_identity::users_by_ids(&room.db, &members_who_may(&shop, PERMISSION_SUPPORT_READ))
            .await?,
    );
    if recipients.is_empty() {
        tracing::debug!(tenant = %tenant_id, message = %message.id, "no member to tell of a message");
        return Ok(Outcome::Dropped);
    }
    if !support
        .claim_notification(
            &message.id,
            owed.expires_at.max(0) as u64,
            claimant(room, now),
        )
        .await?
    {
        return Ok(Outcome::Left);
    }
    let cx = Cx::new(room.mail.clone());
    let link = inbox_link(&room.base_url, &shop.slug);
    // Each member in turn; the first the server fails stops the round, the
    // retry mails everyone again. Addresses refused for good are skipped:
    // the notice went to whoever it could, unless that was nobody.
    let mut told = 0;
    let mut refused = None;
    for to in &recipients {
        let sent = attempt(send_message_notification(
            &cx,
            to,
            &shop.name,
            &message.first_name,
            &link,
        ))
        .await;
        match sent {
            Attempt::Sent => told += 1,
            Attempt::Undeliverable(reason) => {
                tracing::warn!(tenant = %tenant_id, message = %message.id, %reason, "a member's address refused the message notice");
                refused.get_or_insert(reason);
            }
            Attempt::NotSent(reason) => {
                tracing::warn!(tenant = %tenant_id, message = %message.id, %reason, "message notice not sent; deferred");
                support
                    .record_notification_not_sent(&message.id, &room.node, &reason, now)
                    .await?;
                return Ok(Outcome::Deferred);
            }
        }
    }
    match refused {
        Some(reason) if told == 0 => {
            support
                .record_notification_undeliverable(&message.id, &reason)
                .await?;
            Ok(Outcome::Recorded)
        }
        _ => {
            tracing::info!(tenant = %tenant_id, message = %message.id, members = told, "message notice mailed");
            support.record_notification_sent(&message.id).await?;
            Ok(Outcome::Sent)
        }
    }
}

/// The members `permission` lets act: the owners, and those whose role
/// grants it (or implies it).
pub(crate) fn members_who_may(shop: &TenantState, permission: &str) -> Vec<String> {
    shop.members
        .keys()
        .filter(|user_id| shop.can(user_id, permission))
        .cloned()
        .collect()
}

/// The addresses to mail among these users: the living ones that have one
/// (a member who signed up with a provider or a passkey alone has none).
pub(crate) fn mailboxes(users: Vec<UserRow>) -> Vec<String> {
    users
        .into_iter()
        .filter(|user| !user.deleted)
        .filter_map(|user| user.email)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use timada_tenancy::{PERMISSION_SUPPORT_MANAGE, ROLE_OWNER, RoleDef};

    use super::*;

    fn role(permissions: &[&str]) -> RoleDef {
        RoleDef {
            label: String::new(),
            permissions: permissions
                .iter()
                .map(|p| (*p).to_owned())
                .collect::<BTreeSet<_>>(),
        }
    }

    fn user(id: &str, email: Option<&str>, deleted: bool) -> UserRow {
        UserRow {
            id: id.to_owned(),
            account: id.to_owned(),
            label: id.to_owned(),
            is_admin: false,
            deleted,
            registered_at: 0,
            last_sign_in_at: None,
            sign_in_count: 0,
            oauth2_count: 0,
            passkey_count: 0,
            email: email.map(str::to_owned),
            has_password: email.is_some(),
        }
    }

    #[test]
    fn the_members_who_may_read_the_messages_are_told() {
        let mut shop = TenantState::default();
        shop.roles.insert("reader".into(), role(&["support:read"]));
        shop.roles
            .insert("agent".into(), role(&[PERMISSION_SUPPORT_MANAGE]));
        shop.roles.insert("clerk".into(), role(&["orders:read"]));
        for (user, role) in [
            ("owner", ROLE_OWNER),
            ("reader", "reader"),
            ("agent", "agent"),
            ("clerk", "clerk"),
            ("ghost", "no-such-role"),
        ] {
            shop.members.insert(user.into(), role.into());
        }
        assert_eq!(
            members_who_may(&shop, PERMISSION_SUPPORT_READ),
            vec!["agent", "owner", "reader"]
        );
    }

    #[test]
    fn only_living_members_with_an_address_are_mailed() {
        let mailed = mailboxes(vec![
            user("owner", Some("owner@example.com"), false),
            user("oidc-only", None, false),
            user("gone", Some("gone@example.com"), true),
        ]);
        assert_eq!(mailed, vec!["owner@example.com"]);
    }
}

#[cfg(test)]
mod saga_tests {
    use evento::{Evento, Turso};
    use timada_core::Sending;
    use timada_support::ReceiveMessage;
    use timada_tenancy::CreateTenant;
    use topcoat::mail::{Mailbox, MemoryTransport};

    use super::*;
    use crate::code_mails::tests::{mail_context, vault};
    use crate::mail_fault::fake_smtp;
    use crate::mailroom::tests::{BASE_URL, room, room_with, rows, store};

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

    /// A visitor's message to `shop`, sealed under the room's keys.
    async fn message(store: &Turso, room: &Mailroom, shop: &str) -> String {
        timada_support::Command::new(Evento::new(store.clone()), shop)
            .receive_message(
                &*room.keys.for_tenant(shop),
                "storefront",
                ReceiveMessage {
                    first_name: "Ada".to_owned(),
                    last_name: "Lovelace".to_owned(),
                    email: "ada@example.com".to_owned(),
                    body: "Where is my order? It is late.".to_owned(),
                    language: None,
                },
            )
            .await
            .unwrap()
            .id
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

    async fn notice_of(store: &Turso, shop: &str, message: &str) -> Sending {
        timada_support::Command::new(Evento::new(store.clone()), shop)
            .load_message_notification(message)
            .await
            .unwrap()
            .map(|notice| notice.sending)
            .unwrap_or_default()
    }

    /// Two nodes see the message; one tells the owner, with the visitor's
    /// first name and the inbox's link, and the other finds it taken.
    #[tokio::test]
    async fn the_members_are_told_once_however_many_nodes_see_the_message() {
        let store = store().await;
        let transport = MemoryTransport::new();
        let node_a = room(&store, transport.clone(), "node-a");
        let node_b = room(&store, transport.clone(), "node-b");
        let shop = shop(&store).await;
        let message = message(&store, &node_a, &shop).await;
        let now = crate::auth::now();

        assert_eq!(run(&store, &node_a, now).await, 1);
        assert_eq!(run(&store, &node_b, now).await, 0);

        let mails = transport.sent();
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].to(), [Mailbox::new("alice@example.org").unwrap()]);
        assert_eq!(mails[0].subject(), "A message is waiting for Acme Tea");
        let cx = Cx::new(crate::code_mails::tests::mail_context(
            MemoryTransport::new(),
        ));
        let wire = String::from_utf8(mails[0].clone().formatted(&cx).unwrap())
            .unwrap()
            .replace("=\r\n", "");
        assert!(wire.contains("Ada"), "{wire}");
        assert!(!wire.contains("Lovelace"), "{wire}");
        assert!(wire.contains(&inbox_link(BASE_URL, "acme-tea")), "{wire}");
        assert_eq!(notice_of(&store, &shop, &message).await, Sending::Sent);
        pass(&store, &node_a).await;
        pass(&store, &node_b).await;
        assert!(rows(&store).await.is_empty());
    }

    /// A server that does not answer defers the notice; the retry goes out
    /// once due. A message erased meanwhile is nothing to tell.
    #[tokio::test]
    async fn a_server_that_does_not_answer_defers_the_notice_until_the_retry() {
        let store = store().await;
        // The nodes share the message's key, as a cluster's vaults do.
        let keys = vault();
        let down = room_with(
            &store,
            mail_context(fake_smtp::transport(fake_smtp::closed_port())),
            keys.clone(),
            "node-a",
        );
        let shop = shop(&store).await;
        let message = message(&store, &down, &shop).await;
        let now = crate::auth::now();

        assert_eq!(run(&store, &down, now).await, 0);
        let notice = notice_of(&store, &shop, &message).await;
        let Sending::Deferred {
            retry_at,
            attempts: 1,
            ..
        } = notice
        else {
            panic!("{notice:?}");
        };
        let transport = MemoryTransport::new();
        let back = room_with(&store, mail_context(transport.clone()), keys, "node-b");
        assert_eq!(run(&store, &back, retry_at).await, 1);
        assert_eq!(transport.sent().len(), 1);
        assert_eq!(notice_of(&store, &shop, &message).await, Sending::Sent);

        // Erased before anyone could tell of it: dropped.
        let erased = self::message(&store, &back, &shop).await;
        timada_support::Command::new(Evento::new(store.clone()), &shop)
            .forget_message(&*back.keys.for_tenant(&shop), "user-alice", &erased)
            .await
            .unwrap();
        assert_eq!(run(&store, &back, retry_at).await, 0);
        assert_eq!(transport.sent().len(), 1);
        assert!(rows(&store).await.is_empty());
        assert_eq!(notice_of(&store, &shop, &erased).await, Sending::Pending);
    }
}
