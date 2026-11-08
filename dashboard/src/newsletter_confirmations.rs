//! Mails, from the shop, the link that confirms a request for its news: the
//! double opt-in of the newsletter.
//!
//! The command that records a request mails nothing: this subscription sees
//! it on every node, and the node that writes the claim on the subscriber
//! first is the one that mails ([`timada_core::Sending`]), with the link
//! derived again from the subscriber's key, then records how the send went.
//! A server that refuses the address for good leaves `MailUndeliverable`,
//! and the address may ask again at once; anything else fails the pass,
//! which is retried, and the holder of the claim sends again.
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
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::Claimant;
use timada_newsletter::aggregator::Requested;
use topcoat::context::{AppContext, Cx};

use crate::cluster::Executor as Store;
use crate::code_mails::retry;
use crate::key_vault::KeyVault;
use crate::mail::send_newsletter_confirmation;
use crate::mail_fault::SendFailure;
use crate::newsletter::confirm_link;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/newsletter-confirmations";

/// The platform store, where the shop lives.
#[derive(Clone)]
struct Platform(Store);

/// The store, where the subscribers are.
#[derive(Clone)]
struct TenantStore(Evento);

/// What sending needs outside a request.
#[derive(Clone)]
struct Mail(Arc<AppContext>);

/// The tenants' keys, to open the subscribers' addresses and sign the links.
#[derive(Clone)]
struct Keys(Arc<KeyVault>);

/// This node, as the claims name it.
#[derive(Clone)]
struct Node(String);

/// Follows every shop's subscribers. Every node runs it; the node that
/// claims a request's mail first is the one that sends it.
pub fn subscription<E: Executor>(
    platform: Store,
    mail: Arc<AppContext>,
    keys: Arc<KeyVault>,
    store: Evento,
    node: String,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(Mail(mail))
        .data(Keys(keys))
        .data(TenantStore(store))
        .data(Node(node))
        .handler(on_requested())
        // The routing key stays the executor's: this node mails the
        // confirmations of its own region.
        .retry(5)
}

#[evento::subscription]
async fn on_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Requested>,
) -> anyhow::Result<()> {
    let platform: Platform = context.extract();
    let store: TenantStore = context.extract();
    let mail: Mail = context.extract();
    let keys: Keys = context.extract();
    let node: Node = context.extract();
    let tenant = event.tenant.as_str();
    let subscriber = event.aggregate_id.as_str();
    let Some(storefront) = timada_orders::Command::new(store.0.clone(), tenant)
        .storefront_address()
        .await?
    else {
        tracing::warn!(
            tenant,
            subscriber,
            "newsletter confirmation not mailed: the shop has no storefront address; the request is left pending"
        );
        return Ok(());
    };
    let Some(shop) = timada_tenancy::Command(platform.0)
        .load_tenant(tenant)
        .await?
    else {
        return Ok(());
    };
    let newsletter = timada_newsletter::Command::new(store.0, tenant);
    let Some(mail_to) = newsletter
        .claim_confirmation_mail(
            &*keys.0.for_tenant(tenant),
            subscriber,
            event.version,
            Claimant {
                node: &node.0,
                now: crate::auth::now(),
            },
        )
        .await?
    else {
        return Ok(());
    };
    let cx = Cx::new(mail.0);
    match send_newsletter_confirmation(
        &cx,
        &shop,
        &mail_to.email,
        &confirm_link(&storefront, &mail_to.token),
        mail_to.language.as_deref(),
    )
    .await
    {
        Ok(()) => {
            tracing::info!(tenant, subscriber, "newsletter confirmation mailed");
            newsletter
                .record_confirmation_mail_sent(subscriber, event.version)
                .await?;
        }
        Err(err) => match SendFailure::of(err) {
            SendFailure::Undeliverable(reason) => {
                tracing::warn!(tenant, subscriber, %reason, "newsletter confirmation not deliverable");
                newsletter
                    .record_confirmation_mail_undeliverable(subscriber, event.version, &reason)
                    .await?;
            }
            SendFailure::Retry(err) => return Err(retry(err)),
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use evento::Turso;
    use timada_core::Sending;
    use timada_newsletter::{RequestOutcome, RequestSubscription, SOURCE_FORM};
    use topcoat::mail::{Mailbox, MemoryTransport};

    use super::*;
    use crate::code_mails::tests::{TENANT, mail_context, shop, vault};
    use crate::mail_fault::fake_smtp;

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

    async fn pass(
        store: &Turso,
        mail: Arc<AppContext>,
        vault: Arc<KeyVault>,
        node: &str,
    ) -> anyhow::Result<()> {
        subscription::<Turso>(
            Evento::new(store.clone()),
            mail,
            vault,
            Evento::new(store.clone()),
            node.to_owned(),
        )
        .ephemeral()
        .no_retry()
        .run_once(store)
        .await
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
        let store = Turso::in_memory_concurrent().await.unwrap();
        let vault = vault();
        shop(&store).await;
        with_storefront(&store).await;
        let token = request(&store, &vault, "Ada@Example.com", crate::auth::now()).await;
        let transport = MemoryTransport::new();
        let mail = mail_context(transport.clone());

        pass(&store, mail.clone(), vault.clone(), "node-a")
            .await
            .unwrap();
        pass(&store, mail.clone(), vault.clone(), "node-b")
            .await
            .unwrap();
        pass(&store, mail.clone(), vault.clone(), "node-a")
            .await
            .unwrap();

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
    }

    #[tokio::test]
    async fn a_refused_address_is_recorded_and_a_shop_without_storefront_leaves_the_request_pending()
     {
        let store = Turso::in_memory_concurrent().await.unwrap();
        let vault = vault();
        shop(&store).await;
        let now = crate::auth::now();
        request(&store, &vault, "ada@example.com", now).await;
        // No storefront address yet: nothing is claimed, nothing goes out.
        let transport = MemoryTransport::new();
        pass(
            &store,
            mail_context(transport.clone()),
            vault.clone(),
            "node-a",
        )
        .await
        .unwrap();
        assert!(transport.sent().is_empty());
        assert_eq!(sending(&store, "ada@example.com").await, Sending::Pending);

        with_storefront(&store).await;
        let refusing = mail_context(fake_smtp::transport(fake_smtp::serve(
            "550 5.1.1 no such user",
        )));
        pass(&store, refusing, vault.clone(), "node-a")
            .await
            .unwrap();
        assert!(sending(&store, "ada@example.com").await.is_undeliverable());
        // Refused for good: the address asks again at once, and the server
        // down fails the pass for a retry.
        request(&store, &vault, "ada@example.com", now + 1).await;
        let down = mail_context(fake_smtp::transport(fake_smtp::closed_port()));
        pass(&store, down, vault.clone(), "node-a")
            .await
            .expect_err("nobody listens: the pass fails");
        assert!(matches!(
            sending(&store, "ada@example.com").await,
            Sending::Claimed { .. }
        ));
    }
}
