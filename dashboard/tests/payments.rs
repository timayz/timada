//! Payments end to end on one node, with a stand-in for Stripe: a request to
//! pay is answered by the tenant's payments subscription with a payment the
//! storefront can confirm, or refused; the webhook then turns the paid cart
//! into the order, once, and gives back a second payment of the same cart.
//!
//! The dashboard is a binary crate, so its modules are reached through
//! `#[path]` includes, which drag in items this test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/invoicing.rs"]
mod invoicing;
#[path = "../src/limits.rs"]
mod limits;
#[path = "../src/payments/mod.rs"]
mod payments;
#[path = "../src/tenants.rs"]
mod tenants;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use evento::migrator::Migrate;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_catalog::CreateProduct;
use timada_orders::{CartItem, Checkout, PaymentAnswer, STOREFRONT_ACTOR, ShippingAddress};
use timada_tenancy::{
    Command, ConfigurePayments, CreateTenant, LegalIdentity, TenantState, read_model_subscription,
};

use cluster::Cluster;
use config::{PaymentsConfig, TenantsConfig};
use payments::webhook::{Outcome, receive};
use payments::{NewPayment, OpenPayment, PaymentGateway, PaymentStatus, Payments, signature};
use tenants::Tenants;

const WEBHOOK_SECRET: &str = "whsec_test";
const SEAL_KEY: &str = "a-test-secret-of-thirty-two-chars!!";

/// Stripe, as far as these tests need it.
#[derive(Default)]
struct FakeStripe {
    /// Opened payments by idempotency key: the same key, the same payment.
    opened: Mutex<HashMap<String, (OpenPayment, NewPayment)>>,
    /// Payments the shopper paid.
    paid: Mutex<Vec<String>>,
    refunds: Mutex<Vec<(String, u64)>>,
}

impl FakeStripe {
    fn pay(&self, payment_id: &str) {
        self.paid.lock().unwrap().push(payment_id.to_owned());
    }

    fn opened_count(&self) -> usize {
        self.opened.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl PaymentGateway for FakeStripe {
    async fn check_key(&self, secret_key: &str) -> anyhow::Result<()> {
        anyhow::ensure!(secret_key == "sk_test_good", "invalid API key");
        Ok(())
    }

    async fn open_payment(
        &self,
        secret_key: &str,
        payment: NewPayment,
    ) -> anyhow::Result<OpenPayment> {
        anyhow::ensure!(secret_key == "sk_test_good", "invalid API key");
        let mut opened = self.opened.lock().unwrap();
        let count = opened.len();
        let entry = opened
            .entry(payment.idempotency_key.clone())
            .or_insert_with(|| {
                (
                    OpenPayment {
                        payment_id: format!("pi_{count}"),
                        client_secret: format!("pi_{count}_secret"),
                    },
                    payment,
                )
            });
        Ok(entry.0.clone())
    }

    async fn payment_status(
        &self,
        _secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<PaymentStatus> {
        let opened = self.opened.lock().unwrap();
        let (open, new) = opened
            .values()
            .find(|(open, _)| open.payment_id == payment_id)
            .ok_or_else(|| anyhow::anyhow!("no such payment"))?;
        let succeeded = self.paid.lock().unwrap().contains(&open.payment_id);
        Ok(PaymentStatus {
            payment_id: open.payment_id.clone(),
            succeeded,
            amount_received: if succeeded { new.amount_cents } else { 0 },
            currency: new.currency.to_ascii_lowercase(),
            metadata: new.metadata.clone(),
        })
    }

    async fn refund(
        &self,
        _secret_key: &str,
        payment_id: &str,
        amount_cents: u64,
        _idempotency_key: &str,
    ) -> anyhow::Result<String> {
        let mut refunds = self.refunds.lock().unwrap();
        refunds.push((payment_id.to_owned(), amount_cents));
        Ok(format!("re_{}", refunds.len()))
    }
}

struct Harness {
    tenants: Arc<Tenants>,
    platform: Command<cluster::Executor>,
    payments: Arc<Payments>,
    stripe: Arc<FakeStripe>,
    pool: SqlitePool,
    dir: PathBuf,
}

async fn harness() -> Harness {
    let dir = std::env::temp_dir().join(format!("timada-payments-{}", ulid::Ulid::generate()));
    std::fs::create_dir_all(&dir).unwrap();
    let options = SqliteConnectOptions::new()
        .filename(dir.join("platform.db"))
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap();
    {
        let mut conn = pool.acquire().await.unwrap();
        let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
        migrator
            .add_migrations(timada_tenancy::migrations())
            .unwrap();
        migrator
            .run(&mut *conn, &evento::migrator::Plan::apply_all())
            .await
            .unwrap();
    }
    let cluster = Arc::new(Cluster::build(None).await.unwrap());
    let platform = cluster
        .open_platform(None, pool.clone(), pool.clone())
        .await
        .unwrap();
    let stripe = Arc::new(FakeStripe::default());
    let payments = Arc::new(Payments::new(
        Some(&PaymentsConfig {
            secret_key: SEAL_KEY.into(),
            stripe_api_url: None,
        }),
        stripe.clone(),
        platform.clone(),
    ));
    let subscribed = payments.clone();
    let invoicing_platform = platform.clone();
    let tenants = Tenants::new(
        cluster,
        &TenantsConfig {
            dir: dir.join("tenants"),
            max_open: Some(8),
        },
        None,
        platform.clone(),
        pool.clone(),
        vec![
            Arc::new(move |_, tenant_id: &str, executor: &cluster::Executor| {
                payments::subscription(subscribed.clone(), tenant_id, executor.clone())
            }),
            Arc::new(move |_, tenant_id: &str, executor: &cluster::Executor| {
                invoicing::subscription(invoicing_platform.clone(), tenant_id, executor.clone())
            }),
        ],
    );
    Harness {
        tenants,
        platform: Command(platform),
        payments,
        stripe,
        pool,
        dir,
    }
}

fn checkout(expected_total_cents: Option<u64>) -> Checkout {
    Checkout {
        email: "ada@example.org".into(),
        shipping: ShippingAddress {
            name: "Ada".into(),
            line1: "1 rue de la Paix".into(),
            postal_code: "75002".into(),
            city: "Paris".into(),
            country: "FR".into(),
            ..Default::default()
        },
        note: None,
        expected_total_cents,
    }
}

impl Harness {
    async fn create(&self, slug: &str) -> TenantState {
        let tenant = self
            .platform
            .create_tenant(CreateTenant {
                name: slug.into(),
                owner_user_id: "alice".into(),
            })
            .await
            .unwrap();
        read_model_subscription::<cluster::Executor>(self.pool.clone())
            .run_once(&self.platform.0)
            .await
            .unwrap();
        tenant
    }

    /// Says who the merchant is, which a sale needs.
    async fn set_legal(&self, tenant: &TenantState) {
        self.platform
            .set_legal_identity(
                "alice",
                &tenant.id,
                LegalIdentity {
                    legal_name: "Acme SAS".into(),
                    legal_form: "SAS".into(),
                    siret: "12002701600092".into(),
                    line1: "1 rue de la Paix".into(),
                    postal_code: "75002".into(),
                    city: "Paris".into(),
                    country: "FR".into(),
                    contact_email: "shop@example.org".into(),
                    vat_number: Some("FR85120027016".into()),
                    oss_registered: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    /// Sets the legal identity too, unless it is there already.
    async fn connect_stripe(&self, tenant: &TenantState) -> TenantState {
        self.set_legal(tenant).await;
        self.platform
            .configure_payments(
                "alice",
                &tenant.id,
                ConfigurePayments {
                    provider: "stripe".into(),
                    publishable_key: "pk_test_123".into(),
                    secret_key_sealed: self.payments.seal("sk_test_good").unwrap(),
                    webhook_secret_sealed: self.payments.seal(WEBHOOK_SECRET).unwrap(),
                },
            )
            .await
            .unwrap();
        self.platform
            .load_tenant(&tenant.id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn store(&self, tenant: &TenantState) -> cluster::Executor {
        self.tenants.open(tenant).await.unwrap()
    }

    /// A cart holding two mugs at 12.50 EUR; returns the cart and the mug.
    async fn cart(&self, tenant: &TenantState) -> (String, String) {
        let store = self.store(tenant).await;
        let catalog = timada_catalog::Command(store.clone());
        let mug = catalog
            .create_product(
                "alice",
                CreateProduct {
                    name: "Mug".into(),
                    slug: None,
                },
            )
            .await
            .unwrap();
        catalog
            .price_product("alice", &mug.id, 1250, "EUR")
            .await
            .unwrap();
        catalog.publish_product("alice", &mug.id).await.unwrap();
        let orders = timada_orders::Command(store);
        let cart_id = orders.create_cart(STOREFRONT_ACTOR).await.unwrap();
        orders
            .add_to_cart(
                STOREFRONT_ACTOR,
                &cart_id,
                CartItem {
                    product_id: mug.id.clone(),
                    variant_id: None,
                    quantity: 2,
                },
            )
            .await
            .unwrap();
        (cart_id, mug.id)
    }

    /// Asks to pay, as a storefront does, and waits for the answer.
    async fn ask(
        &self,
        tenant: &TenantState,
        cart_id: &str,
        expected: Option<u64>,
    ) -> PaymentAnswer {
        let orders = timada_orders::Command(self.store(tenant).await);
        let request_id = orders
            .request_checkout(STOREFRONT_ACTOR, cart_id, checkout(expected))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let cart = orders.load_cart(cart_id).await.unwrap().unwrap();
                if let Some(answer) = cart.request(&request_id).unwrap().answer.clone() {
                    return answer;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the request to pay is answered")
    }

    async fn webhook(&self, tenant: &TenantState, body: &str, header: Option<String>) -> Outcome {
        let tenant = self
            .platform
            .load_tenant(&tenant.id)
            .await
            .unwrap()
            .unwrap();
        let account = self.payments.account_of(&tenant).unwrap().unwrap();
        receive(
            &self.payments,
            &account,
            &tenant.id,
            self.store(&tenant).await,
            body.as_bytes(),
            header.as_deref(),
            now(),
        )
        .await
        .unwrap()
    }

    async fn finish(self) {
        self.tenants.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn succeeded(payment_id: &str) -> String {
    format!(
        r#"{{"id":"evt_1","type":"payment_intent.succeeded","data":{{"object":{{"id":"{payment_id}","object":"payment_intent"}}}}}}"#
    )
}

fn signed(body: &str) -> Option<String> {
    Some(signature::sign(body.as_bytes(), WEBHOOK_SECRET, now()))
}

#[tokio::test]
async fn a_paid_cart_becomes_one_order() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, _) = h.cart(&acme).await;

    let PaymentAnswer::Requested(payment) = h.ask(&acme, &cart_id, Some(2500)).await else {
        panic!("a payment is opened");
    };
    assert_eq!(payment.client_secret, "pi_0_secret");
    assert_eq!(payment.publishable_key, "pk_test_123");
    assert_eq!(payment.total_cents, 2500);
    {
        let opened = h.stripe.opened.lock().unwrap();
        let (_, new) = opened.values().next().unwrap();
        assert_eq!(new.amount_cents, 2500);
        assert_eq!(new.metadata["tenant_id"], acme.id);
        assert_eq!(new.metadata["cart_id"], cart_id);
        assert_eq!(new.receipt_email.as_deref(), Some("ada@example.org"));
    }

    // Not paid yet: Stripe's word is that it did not succeed.
    let body = succeeded(&payment.payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Ignored
    );
    h.stripe.pay(&payment.payment_id);
    let Outcome::Placed { order_id } = h.webhook(&acme, &body, signed(&body)).await else {
        panic!("the order is placed");
    };
    // Told again: the same order, nothing more.
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Placed {
            order_id: order_id.clone()
        }
    );
    let orders = timada_orders::Command(h.store(&acme).await);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 2500);
    assert_eq!(order.payment.payment_id, payment.payment_id);
    assert_eq!(order.payment.currency, "EUR");
    assert_eq!(order.email, "ada@example.org");

    // The order is invoiced, at the merchant's default rate.
    let invoicing = timada_invoicing::Command(h.store(&acme).await);
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    assert!(invoice.label.ends_with("-000001"), "{}", invoice.label);
    assert!(invoice.label.starts_with('F'));
    assert_eq!(invoice.total_cents, 2500);
    assert_eq!(invoice.lines[0].vat_rate_bps, Some(2000));
    assert_eq!(invoice.vat[0].base_cents, 2083);
    assert_eq!(invoice.vat[0].vat_cents, 417);
    assert_eq!(invoice.seller.address.name, "Acme SAS");
    assert_eq!(invoice.buyer.name, "Ada");
    assert_eq!(invoice.order_reference, order.reference);
    assert!(
        timada_invoicing::pdf::render(&invoice)
            .unwrap()
            .starts_with(b"%PDF-")
    );

    // A refund is credited against it.
    orders
        .record_refund("alice", &order_id, "re_42", 1000)
        .await
        .unwrap();
    let credit =
        eventually(async || invoicing.load_credit_note_of_refund("re_42").await.unwrap()).await;
    assert!(credit.label.starts_with('A') && credit.label.ends_with("-000001"));
    assert_eq!(
        credit.invoice_label.as_deref(),
        Some(invoice.label.as_str())
    );
    assert_eq!(credit.total_cents, 1000);
    h.finish().await;
}

/// Waits for what a subscription writes.
async fn eventually<T>(mut probe: impl AsyncFnMut() -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(value) = probe().await {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the subscription wrote it")
}

#[tokio::test]
async fn webhooks_that_do_not_check_out_change_nothing() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, _) = h.cart(&acme).await;
    let PaymentAnswer::Requested(payment) = h.ask(&acme, &cart_id, None).await else {
        panic!("a payment is opened");
    };
    h.stripe.pay(&payment.payment_id);
    let body = succeeded(&payment.payment_id);

    assert_eq!(h.webhook(&acme, &body, None).await, Outcome::Rejected);
    let forged = Some(signature::sign(body.as_bytes(), "whsec_guess", now()));
    assert_eq!(h.webhook(&acme, &body, forged).await, Outcome::Rejected);
    let stale = Some(signature::sign(
        body.as_bytes(),
        WEBHOOK_SECRET,
        now() - 3600,
    ));
    assert_eq!(h.webhook(&acme, &body, stale).await, Outcome::Rejected);
    let other = r#"{"id":"evt_2","type":"charge.refunded","data":{"object":{"id":"ch_1"}}}"#;
    assert_eq!(
        h.webhook(&acme, other, signed(other)).await,
        Outcome::Ignored
    );

    let orders = timada_orders::Command(h.store(&acme).await);
    assert!(orders.load_order_of_cart(&cart_id).await.unwrap().is_none());
    h.finish().await;
}

#[tokio::test]
async fn requests_that_cannot_go_ahead_are_refused() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let (cart_id, mug) = h.cart(&acme).await;

    // Nobody to name on the invoice yet.
    assert_eq!(
        h.ask(&acme, &cart_id, None).await,
        PaymentAnswer::Refused("invoicing_not_configured".into())
    );
    // No Stripe account yet.
    h.set_legal(&acme).await;
    assert_eq!(
        h.ask(&acme, &cart_id, None).await,
        PaymentAnswer::Refused("payments_unavailable".into())
    );
    // The price moved since the shopper saw 25.00.
    h.connect_stripe(&acme).await;
    timada_catalog::Command(h.store(&acme).await)
        .price_product("alice", &mug, 1300, "EUR")
        .await
        .unwrap();
    assert_eq!(
        h.ask(&acme, &cart_id, Some(2500)).await,
        PaymentAnswer::Refused("price_changed".into())
    );
    assert_eq!(h.stripe.opened_count(), 0, "nothing was opened with Stripe");
    h.finish().await;
}

#[tokio::test]
async fn a_second_payment_of_a_cart_is_given_back() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, _) = h.cart(&acme).await;
    // Two tabs: two payments opened, both paid.
    let PaymentAnswer::Requested(first) = h.ask(&acme, &cart_id, None).await else {
        panic!("a payment is opened");
    };
    let PaymentAnswer::Requested(second) = h.ask(&acme, &cart_id, None).await else {
        panic!("a payment is opened");
    };
    assert_ne!(first.payment_id, second.payment_id);
    h.stripe.pay(&first.payment_id);
    h.stripe.pay(&second.payment_id);

    let body = succeeded(&first.payment_id);
    assert!(matches!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Placed { .. }
    ));
    let body = succeeded(&second.payment_id);
    assert!(matches!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Refunded { .. }
    ));
    assert_eq!(
        *h.stripe.refunds.lock().unwrap(),
        vec![(second.payment_id.clone(), 2500)]
    );
    h.finish().await;
}

#[tokio::test]
async fn the_stripe_secrets_are_kept_sealed() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let settings = acme.payments.clone().unwrap();
    assert!(
        !settings
            .secret_key_sealed
            .windows(b"sk_test_good".len())
            .any(|window| window == b"sk_test_good")
    );
    let account = h.payments.account_of(&acme).unwrap().unwrap();
    assert_eq!(account.secret_key, "sk_test_good");
    assert_eq!(account.webhook_secret, WEBHOOK_SECRET);

    let other_node = Payments::new(
        Some(&PaymentsConfig {
            secret_key: "another-secret-of-thirty-two-chars!".into(),
            stripe_api_url: None,
        }),
        h.stripe.clone(),
        h.platform.0.clone(),
    );
    assert!(other_node.account_of(&acme).is_err());
    let disabled = Payments::new(None, h.stripe.clone(), h.platform.0.clone());
    assert!(disabled.account_of(&acme).unwrap().is_none());
    h.finish().await;
}

/// The real client against stripe-mock (`docker compose up -d stripe-mock`),
/// when `STRIPE_MOCK_URL` says where it is: each call the dashboard makes
/// is sent and its answer read back as Stripe shapes it. Skipped otherwise.
#[tokio::test]
async fn the_stripe_client_speaks_stripe() {
    let Ok(url) = std::env::var("STRIPE_MOCK_URL") else {
        eprintln!("STRIPE_MOCK_URL is not set: skipped");
        return;
    };
    let stripe = payments::StripeGateway::new(Some(url));
    stripe.check_key("sk_test_123").await.unwrap();
    let opened = stripe
        .open_payment(
            "sk_test_123",
            NewPayment {
                amount_cents: 2500,
                currency: "EUR".into(),
                idempotency_key: format!("timada-test-{}", ulid::Ulid::generate()),
                metadata: HashMap::from([("cart_id".to_owned(), "c1".to_owned())]),
                receipt_email: Some("ada@example.org".into()),
                description: None,
            },
        )
        .await
        .unwrap();
    assert!(opened.payment_id.starts_with("pi_"), "{opened:?}");
    assert!(opened.client_secret.contains("_secret_"), "{opened:?}");
    let status = stripe
        .payment_status("sk_test_123", &opened.payment_id)
        .await
        .unwrap();
    assert!(status.payment_id.starts_with("pi_"));
    let refund = stripe
        .refund(
            "sk_test_123",
            &opened.payment_id,
            2500,
            "timada-test-refund",
        )
        .await
        .unwrap();
    assert!(refund.starts_with("re_"), "{refund}");
}
