//! Mails the invitations to join a tenant: a platform mail with the link
//! whose token the stream's nonce derives under the host's key.
//!
//! The command that records an invitation mails nothing: this subscription
//! sees it on every node, and the node that writes the claim on the
//! invitation first is the one that mails ([`timada_core::Sending`]), then
//! records how the send went. A server that refuses the address for good
//! leaves `MailUndeliverable`, which the members page shows, so the inviter
//! sends a corrected one; anything else fails the pass, which is retried,
//! and the holder of the claim sends again.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::Claimant;
use timada_tenancy::aggregator::Sent;
use topcoat::context::{AppContext, Cx, app_context};

use crate::app::t::slug::role_label;
use crate::cluster::Executor as Store;
use crate::code_mails::retry;
use crate::mail::send_invitation;
use crate::mail_fault::SendFailure;
use crate::seal::{EmailCodes, InvitationTokens};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/invitation-mails";

/// The platform store, where the invitations, the tenants and the users
/// live.
#[derive(Clone)]
struct Platform(Store);

/// What sending needs outside a request: the platform's transport and
/// sender, and the key the links derive under.
#[derive(Clone)]
struct Mail(Arc<AppContext>);

/// The dashboard's public address, for the link.
#[derive(Clone)]
struct BaseUrl(String);

/// This node, as the claims name it.
#[derive(Clone)]
struct Node(String);

/// Follows the platform's invitations. Every node runs it; the node that
/// claims an invitation's mail first is the one that sends it.
pub fn subscription<E: Executor>(
    platform: Store,
    mail: Arc<AppContext>,
    base_url: String,
    node: String,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(Mail(mail))
        .data(BaseUrl(base_url))
        .data(Node(node))
        .handler(on_sent())
        // The routing key stays the executor's: this node mails the
        // invitations of its own region.
        .tenant("")
        .retry(5)
}

/// The page of the dashboard at `base_url` that `token` opens.
pub fn invitation_link(base_url: &str, token: &str) -> String {
    format!("{}/invitations/{token}", base_url.trim_end_matches('/'))
}

#[evento::subscription]
async fn on_sent<E: Executor>(context: &Context<'_, E>, event: Event<Sent>) -> anyhow::Result<()> {
    let platform: Platform = context.extract();
    let mail: Mail = context.extract();
    let base_url: BaseUrl = context.extract();
    let node: Node = context.extract();
    let cx = Cx::new(mail.0);
    let tenancy = timada_tenancy::Command(platform.0.clone());
    // The tenant and the inviter first: gone, there is nothing to send.
    let Some(tenant) = tenancy.load_tenant(&event.data.tenant_id).await? else {
        return Ok(());
    };
    let inviter =
        timada_identity::Command::new(platform.0, app_context::<EmailCodes>(&cx).0.clone())
            .load_user(&event.data.invited_by)
            .await?
            .map_or_else(|| "A member".to_owned(), |user| user.label);
    let invitation = event.aggregate_id.as_str();
    let Some(mail) = tenancy
        .claim_invitation_mail(
            &*app_context::<InvitationTokens>(&cx).0,
            invitation,
            Claimant {
                node: &node.0,
                now: crate::auth::now(),
            },
        )
        .await?
    else {
        return Ok(());
    };
    let link = invitation_link(&base_url.0, &mail.token);
    let role = role_label(&tenant, &mail.invitation.role);
    match send_invitation(
        &cx,
        &mail.invitation.email,
        &tenant.name,
        &inviter,
        &role,
        &link,
    )
    .await
    {
        Ok(()) => {
            tracing::info!(invitation, tenant = %tenant.id, "invitation mailed");
            tenancy.record_invitation_mail_sent(invitation).await?;
        }
        Err(err) => match SendFailure::of(err) {
            SendFailure::Undeliverable(reason) => {
                tracing::warn!(invitation, tenant = %tenant.id, %reason, "invitation not deliverable");
                tenancy
                    .record_invitation_mail_undeliverable(invitation, &reason)
                    .await?;
            }
            SendFailure::Retry(err) => return Err(retry(err)),
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use evento::{Evento, Turso};
    use timada_core::Sending;
    use timada_tenancy::{InviteMember, ROLE_OWNER};
    use topcoat::mail::{Mailbox, MemoryTransport};

    use super::*;
    use crate::code_mails::tests::{TENANT, mail_context, shop};
    use crate::mail_fault::fake_smtp;

    const BASE: &str = "https://timada.example/";

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

    async fn pass(store: &Turso, mail: Arc<AppContext>, node: &str) -> anyhow::Result<()> {
        subscription::<Turso>(
            Evento::new(store.clone()),
            mail,
            BASE.to_owned(),
            node.to_owned(),
        )
        .ephemeral()
        .no_retry()
        .run_once(store)
        .await
    }

    #[tokio::test]
    async fn an_invitation_is_mailed_once_with_the_link_the_token_opens() {
        let store = Turso::in_memory_concurrent().await.unwrap();
        shop_with_owner(&store).await;
        let sent = invite(&store, "Bob@Example.com").await;
        let transport = MemoryTransport::new();
        let mail = mail_context(transport.clone());

        pass(&store, mail.clone(), "node-a").await.unwrap();
        pass(&store, mail.clone(), "node-b").await.unwrap();
        pass(&store, mail.clone(), "node-a").await.unwrap();

        let mails = transport.sent();
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].to(), [Mailbox::new("bob@example.com").unwrap()]);
        assert_eq!(
            mails[0].subject(),
            "You are invited to join Acme Tea on Timada"
        );
        let cx = Cx::new(mail);
        // The wire folds long lines.
        let wire = String::from_utf8(mails[0].clone().formatted(&cx).unwrap())
            .unwrap()
            .replace("=\r\n", "");
        assert!(
            wire.contains(&format!(
                "https://timada.example/invitations/{}",
                sent.token
            )),
            "{wire}"
        );
        assert!(wire.contains("Alice"), "{wire}");
        assert!(wire.contains("owner"), "{wire}");
        let tenancy = timada_tenancy::Command(Evento::new(store.clone()));
        let state = tenancy
            .load_invitation(&sent.invitation.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.mail, Sending::Sent);
        // The link in the mail is the one the acceptance knows.
        assert!(
            tenancy
                .load_invitation_by_token(&sent.token)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_refused_address_is_recorded_and_a_down_server_fails_the_pass() {
        let store = Turso::in_memory_concurrent().await.unwrap();
        shop_with_owner(&store).await;
        let refused = invite(&store, "bob@example.com").await;
        let mail = mail_context(fake_smtp::transport(fake_smtp::serve(
            "550 5.1.1 no such user",
        )));
        pass(&store, mail, "node-a").await.unwrap();
        let tenancy = timada_tenancy::Command(Evento::new(store.clone()));
        let state = tenancy
            .load_invitation(&refused.invitation.id)
            .await
            .unwrap()
            .unwrap();
        assert!(state.mail.is_undeliverable(), "{:?}", state.mail);

        let pending = invite(&store, "carol@example.com").await;
        let down = mail_context(fake_smtp::transport(fake_smtp::closed_port()));
        pass(&store, down, "node-a")
            .await
            .expect_err("nobody listens: the pass fails");
        let state = tenancy
            .load_invitation(&pending.invitation.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(&state.mail, Sending::Claimed { by, .. } if by == "node-a"),
            "{:?}",
            state.mail
        );
        // Withdrawn before the server is back: nothing goes out.
        tenancy
            .revoke_invitation("user-alice", &pending.invitation.id, crate::auth::now())
            .await
            .unwrap();
        let transport = MemoryTransport::new();
        pass(&store, mail_context(transport.clone()), "node-a")
            .await
            .unwrap();
        assert!(transport.sent().is_empty());
    }
}
