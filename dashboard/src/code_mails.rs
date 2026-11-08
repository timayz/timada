//! Mails the one-time codes: the platform's, to prove an address when
//! signing up, signing in or resetting a password, and the shops', to their
//! shoppers under the shop's name.
//!
//! The command that issues a code mails nothing: it records the intent, and
//! these subscriptions see it on every node. The node that writes the
//! claim on the challenge first is the one that mails
//! ([`timada_core::Sending`]); it derives the code from the nonce the event
//! carries, under the key the host holds, and records how the send went. A
//! server that refuses the address for good leaves `MailUndeliverable`, and
//! the person may ask again at once; anything else fails the pass, which is
//! retried, and the holder of the claim sends again.

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::Claimant;
use topcoat::context::{AppContext, Cx, app_context};

use crate::cluster::Executor as Store;
use crate::key_vault::KeyVault;
use crate::mail::{send_code, send_shopper_code};
use crate::mail_fault::SendFailure;
use crate::seal::{EmailCodes, ShopperCodes};

/// The cursor identity of the subscription over the platform's codes, in
/// the store.
pub const PLATFORM_SUBSCRIPTION: &str = "timada-dashboard/platform-code-mails";
/// The cursor identity of the subscription over the shops' codes.
pub const SHOPPER_SUBSCRIPTION: &str = "timada-dashboard/shopper-code-mails";

/// The platform store, where the users' challenges and the shops live.
#[derive(Clone)]
struct Platform(Store);

/// The store, where the shoppers' challenges live.
#[derive(Clone)]
struct TenantStore(Evento);

/// What sending needs outside a request: the platform's transport and
/// sender, the shops' own servers, and the keys the codes derive under.
#[derive(Clone)]
struct Mail(Arc<AppContext>);

/// The tenants' keys, to open the shoppers' addresses.
#[derive(Clone)]
struct Keys(Arc<KeyVault>);

/// This node, as the claims name it.
#[derive(Clone)]
struct Node(String);

/// Follows the platform's challenges. Every node runs it; the node that
/// claims a code's mail first is the one that sends it.
pub fn platform_subscription<E: Executor>(
    platform: Store,
    mail: Arc<AppContext>,
    node: String,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PLATFORM_SUBSCRIPTION)
        .data(Platform(platform))
        .data(Mail(mail))
        .data(Node(node))
        .handler(on_email_code_issued())
        // The routing key stays the executor's: this node mails the codes
        // of its own region.
        .tenant("")
        .retry(5)
}

/// Follows every shop's challenges, the same way.
pub fn shopper_subscription<E: Executor>(
    platform: Store,
    mail: Arc<AppContext>,
    keys: Arc<KeyVault>,
    store: Evento,
    node: String,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SHOPPER_SUBSCRIPTION)
        .data(Platform(platform))
        .data(Mail(mail))
        .data(Keys(keys))
        .data(TenantStore(store))
        .data(Node(node))
        .handler(on_shopper_code_issued())
        // The routing key stays the executor's: this node mails the codes
        // of its own region.
        .retry(5)
}

#[evento::subscription]
async fn on_email_code_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_identity::aggregator::CodeIssued>,
) -> anyhow::Result<()> {
    let platform: Platform = context.extract();
    let mail: Mail = context.extract();
    let node: Node = context.extract();
    let cx = Cx::new(mail.0);
    let identity =
        timada_identity::Command::new(platform.0, app_context::<EmailCodes>(&cx).0.clone());
    let challenge = event.aggregate_id.as_str();
    let Some(code) = identity
        .claim_code_mail(challenge, event.version, claimant(&node))
        .await?
    else {
        return Ok(());
    };
    match send_code(&cx, &code.email, code.purpose, &code.code).await {
        Ok(()) => {
            tracing::info!(challenge, purpose = ?code.purpose, "sign-in code mailed");
            identity
                .record_code_mail_sent(challenge, event.version)
                .await?;
        }
        Err(err) => match SendFailure::of(err) {
            SendFailure::Undeliverable(reason) => {
                tracing::warn!(challenge, %reason, "sign-in code not deliverable");
                identity
                    .record_code_mail_undeliverable(challenge, event.version, &reason)
                    .await?;
            }
            SendFailure::Retry(err) => return Err(retry(err)),
        },
    }
    Ok(())
}

#[evento::subscription]
async fn on_shopper_code_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::CodeIssued>,
) -> anyhow::Result<()> {
    let mail: Mail = context.extract();
    let cx = Cx::new(mail.0);
    // Without `secrets.key` no code could be issued through this node, and
    // none can be derived.
    let Some(codes) = app_context::<ShopperCodes>(&cx).0.clone() else {
        return Ok(());
    };
    let platform: Platform = context.extract();
    let store: TenantStore = context.extract();
    let keys: Keys = context.extract();
    let node: Node = context.extract();
    let tenant = event.tenant.as_str();
    let Some(shop) = timada_tenancy::Command(platform.0)
        .load_tenant(tenant)
        .await?
    else {
        return Ok(());
    };
    let shoppers = timada_shoppers::Command::new(store.0, tenant);
    let challenge = event.aggregate_id.as_str();
    let Some(code) = shoppers
        .claim_code_mail(
            &*keys.0.for_tenant(tenant),
            &*codes,
            challenge,
            event.version,
            claimant(&node),
        )
        .await?
    else {
        return Ok(());
    };
    match send_shopper_code(
        &cx,
        &code.email,
        &shop,
        &code.code,
        code.language.as_deref(),
    )
    .await
    {
        Ok(()) => {
            tracing::info!(tenant, challenge, "shopper sign-in code mailed");
            shoppers
                .record_code_mail_sent(challenge, event.version)
                .await?;
        }
        Err(err) => match SendFailure::of(err) {
            SendFailure::Undeliverable(reason) => {
                tracing::warn!(tenant, challenge, %reason, "shopper sign-in code not deliverable");
                shoppers
                    .record_code_mail_undeliverable(challenge, event.version, &reason)
                    .await?;
            }
            SendFailure::Retry(err) => return Err(retry(err)),
        },
    }
    Ok(())
}

/// This node, asking now.
fn claimant(node: &Node) -> Claimant<'_> {
    Claimant {
        node: &node.0,
        now: crate::auth::now(),
    }
}

/// A send failure worth a retry, as the pass fails with it.
pub(crate) fn retry(err: topcoat::Error) -> anyhow::Error {
    anyhow::anyhow!("mail not sent: {err:#}")
}

#[cfg(test)]
pub(crate) mod tests {
    use evento::Turso;
    use timada_core::Sending;
    use timada_identity::ChallengePurpose;
    use topcoat::mail::{MailConfig, Mailbox, MemoryTransport, Transport};

    use super::*;
    use crate::key_vault::KeyVault;
    use crate::mail::MailSender;
    use crate::mail_fault::fake_smtp;
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

    async fn platform_pass(store: &Turso, mail: Arc<AppContext>, node: &str) -> anyhow::Result<()> {
        platform_subscription::<Turso>(Evento::new(store.clone()), mail, node.to_owned())
            .ephemeral()
            .no_retry()
            .run_once(store)
            .await
    }

    async fn shopper_pass(
        store: &Turso,
        mail: Arc<AppContext>,
        vault: Arc<KeyVault>,
        node: &str,
    ) -> anyhow::Result<()> {
        shopper_subscription::<Turso>(
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

    #[tokio::test]
    async fn a_platform_code_is_mailed_once_however_many_nodes_see_it() {
        let store = Turso::in_memory_concurrent().await.unwrap();
        let identity = identity(&store);
        let issued = identity
            .issue_email_code(
                "alice@example.com",
                ChallengePurpose::Register,
                crate::auth::now(),
            )
            .await
            .unwrap();
        let transport = MemoryTransport::new();
        let mail = mail_context(transport.clone());

        platform_pass(&store, mail.clone(), "node-a").await.unwrap();
        platform_pass(&store, mail.clone(), "node-b").await.unwrap();
        // The event delivered again to the sender: nothing more goes out.
        platform_pass(&store, mail, "node-a").await.unwrap();

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
    }

    #[tokio::test]
    async fn a_refused_address_is_recorded_and_the_person_may_try_again() {
        let store = Turso::in_memory_concurrent().await.unwrap();
        let identity = identity(&store);
        let now = crate::auth::now();
        identity
            .issue_email_code("alice@example.com", ChallengePurpose::Register, now)
            .await
            .unwrap();
        let mail = mail_context(fake_smtp::transport(fake_smtp::serve(
            "550 5.1.1 no such user",
        )));

        platform_pass(&store, mail, "node-a").await.unwrap();

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

    #[tokio::test]
    async fn a_server_that_cannot_be_reached_fails_the_pass_for_a_retry() {
        let store = Turso::in_memory_concurrent().await.unwrap();
        let identity = identity(&store);
        identity
            .issue_email_code(
                "alice@example.com",
                ChallengePurpose::Register,
                crate::auth::now(),
            )
            .await
            .unwrap();
        let down = mail_context(fake_smtp::transport(fake_smtp::closed_port()));
        platform_pass(&store, down, "node-a")
            .await
            .expect_err("nobody listens: the pass fails");
        let challenge = identity
            .load_email_challenge("alice@example.com")
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(&challenge.mail, Sending::Claimed { by, .. } if by == "node-a"),
            "{:?}",
            challenge.mail
        );
        // A transient refusal, too.
        let busy = mail_context(fake_smtp::transport(fake_smtp::serve(
            "450 4.2.0 try again later",
        )));
        platform_pass(&store, busy, "node-a")
            .await
            .expect_err("a 4xx is worth a retry");

        // The server back, the holder of the claim sends.
        let transport = MemoryTransport::new();
        platform_pass(&store, mail_context(transport.clone()), "node-a")
            .await
            .unwrap();
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
    }

    #[tokio::test]
    async fn a_shopper_code_comes_from_the_shop_in_their_language_once() {
        let store = Turso::in_memory_concurrent().await.unwrap();
        let vault = vault();
        shop(&store).await;
        let shoppers = timada_shoppers::Command::new(Evento::new(store.clone()), TENANT);
        let issued = shoppers
            .issue_code(
                &*vault.for_tenant(TENANT),
                &*shopper_codes(),
                "Ada@Example.com",
                Some("fr-FR"),
                crate::auth::now(),
            )
            .await
            .unwrap();
        let transport = MemoryTransport::new();
        let mail = mail_context(transport.clone());

        shopper_pass(&store, mail.clone(), vault.clone(), "node-a")
            .await
            .unwrap();
        shopper_pass(&store, mail.clone(), vault.clone(), "node-b")
            .await
            .unwrap();

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
    async fn a_shoppers_refused_address_is_recorded() {
        let store = Turso::in_memory_concurrent().await.unwrap();
        let vault = vault();
        shop(&store).await;
        let shoppers = timada_shoppers::Command::new(Evento::new(store.clone()), TENANT);
        shoppers
            .issue_code(
                &*vault.for_tenant(TENANT),
                &*shopper_codes(),
                "ada@example.com",
                None,
                crate::auth::now(),
            )
            .await
            .unwrap();
        let refusing = mail_context(fake_smtp::transport(fake_smtp::serve(
            "553 mailbox name not allowed",
        )));
        shopper_pass(&store, refusing, vault.clone(), "node-a")
            .await
            .unwrap();
        assert!(
            shoppers
                .load_challenge("ada@example.com")
                .await
                .unwrap()
                .unwrap()
                .mail
                .is_undeliverable()
        );
        // A later code, with the server down, fails the pass instead.
        shoppers
            .issue_code(
                &*vault.for_tenant(TENANT),
                &*shopper_codes(),
                "ada@example.com",
                None,
                crate::auth::now() + 1,
            )
            .await
            .unwrap();
        let down = mail_context(fake_smtp::transport(fake_smtp::closed_port()));
        shopper_pass(&store, down, vault, "node-a")
            .await
            .expect_err("nobody listens: the pass fails");
    }
}
