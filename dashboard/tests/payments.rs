//! Payments end to end on one node, with a stand-in for Stripe: a request to
//! pay is answered by the tenant's payments subscription with a payment the
//! storefront can confirm, or refused; the webhook then turns the confirmed
//! cart into the order, once, its money held, and gives back a second
//! payment of the same cart. The money of goods the shop holds is taken at
//! once; a cancelled order's hold is given back.
//!
//! The dashboard is a binary crate, so its modules are reached through
//! `#[path]` includes, which drag in items this test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/db.rs"]
mod db;
#[path = "../src/ecb.rs"]
mod ecb;
#[path = "../src/invoicing.rs"]
mod invoicing;
#[path = "../src/payments/mod.rs"]
mod payments;
#[path = "../src/seal.rs"]
mod seal;
#[path = "../src/vies.rs"]
mod vies;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use evento::ProjectionAggregate;
use timada_catalog::{CreateProduct, StockLine};
use timada_discounts::RESERVATION_SECS;
use timada_orders::{
    CartCursor, CartItem, Checkout, PaymentAnswer, STOREFRONT_ACTOR, ShippingAddress,
};
use timada_tenancy::{
    Command, ConfigurePayments, CreateTenant, LegalIdentity, TenantState, read_model_subscription,
};

use cluster::Cluster;
use payments::settle::{CancelOutcome, CaptureOutcome};
use payments::webhook::{Outcome, receive};
use payments::{
    ASKED_SECS, Busy, Cancellation, CaptureAnswer, Checkouts, DASHBOARD_ACTOR, LapsedCheckouts,
    NewPayment, OpenPayment, PaymentGateway, PaymentState, PaymentStatus, Payments,
    PendingPayments, RECHECK_SECS, RefundRecord, Refused, ReleaseAnswer, SuppliedGoods,
    UnansweredCheckouts, signature,
};
use timada_orders::{CaptureClaim, HOLD_EXPIRED_REASON, OrderLine, OrderStatus};

const WEBHOOK_SECRET: &str = "whsec_test";
const SEAL_KEY: &str = "a-test-secret-of-thirty-two-chars!!";

/// Stripe, as far as these tests need it.
#[derive(Default)]
struct FakeStripe {
    /// Opened payments by idempotency key: the same key, the same payment.
    opened: Mutex<HashMap<String, (OpenPayment, NewPayment)>>,
    /// Payments the shopper confirmed: the money is held on their card.
    authorized: Mutex<Vec<String>>,
    /// Payments whose money was taken.
    captured: Mutex<Vec<String>>,
    /// Payments confirmed that Stripe is settling (a bank debit).
    confirming: Mutex<Vec<String>>,
    /// Payments cancelled, by this server or in Stripe's own dashboard.
    cancelled: Mutex<Vec<String>>,
    /// Payments Stripe cancelled on its own, their hold lapsed.
    lapsed: Mutex<Vec<String>>,
    refunds: Mutex<Vec<(String, u64)>>,
    /// Stripe is down: opening a payment fails.
    down: std::sync::atomic::AtomicBool,
    /// Taking a held payment is refused (the card was cancelled, say).
    refuses_capture: std::sync::atomic::AtomicBool,
    /// Taking or giving back a hold gets no answer.
    unreachable: std::sync::atomic::AtomicBool,
    /// Calls to open a payment, whatever their answer.
    open_calls: std::sync::atomic::AtomicUsize,
    /// How long opening a payment takes, in milliseconds.
    open_delay_ms: std::sync::atomic::AtomicU64,
    /// The idempotency keys of the payments being opened: a second call
    /// with one of them meanwhile is answered 409, as Stripe does.
    opening: Mutex<std::collections::HashSet<String>>,
    /// That many calls to open a payment are answered 409 first.
    busy_opens: std::sync::atomic::AtomicUsize,
    /// Refunds are refused for good (a disputed charge).
    refuses_refund: std::sync::atomic::AtomicBool,
    /// Calls to refund, whatever their answer.
    refund_attempts: std::sync::atomic::AtomicUsize,
}

impl FakeStripe {
    /// The shopper confirms the payment: its money is held.
    fn authorize(&self, payment_id: &str) {
        self.authorized.lock().unwrap().push(payment_id.to_owned());
    }

    /// The hold lapses: Stripe cancels the payment on its own.
    fn lapse(&self, payment_id: &str) {
        self.lapsed.lock().unwrap().push(payment_id.to_owned());
    }

    fn opened_count(&self) -> usize {
        self.opened.lock().unwrap().len()
    }

    fn amount_of(&self, payment_id: &str) -> u64 {
        self.opened
            .lock()
            .unwrap()
            .values()
            .find(|(open, _)| open.payment_id == payment_id)
            .map_or(0, |(_, new)| new.amount_cents)
    }

    fn is(&self, list: &Mutex<Vec<String>>, payment_id: &str) -> bool {
        list.lock().unwrap().iter().any(|id| id == payment_id)
    }

    fn state_of(&self, payment_id: &str) -> PaymentState {
        if self.is(&self.captured, payment_id) {
            PaymentState::Captured
        } else if self.is(&self.cancelled, payment_id) || self.is(&self.lapsed, payment_id) {
            PaymentState::Cancelled
        } else if self.is(&self.authorized, payment_id) {
            PaymentState::Authorized
        } else if self.is(&self.confirming, payment_id) {
            PaymentState::Processing
        } else {
            PaymentState::Open
        }
    }

    fn reachable(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.unreachable.load(std::sync::atomic::Ordering::SeqCst),
            "Stripe did not answer"
        );
        Ok(())
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
        use std::sync::atomic::Ordering::SeqCst;
        self.open_calls.fetch_add(1, SeqCst);
        anyhow::ensure!(secret_key == "sk_test_good", "invalid API key");
        anyhow::ensure!(!self.down.load(SeqCst), "Stripe is down");
        let in_use = || {
            anyhow::Error::new(Busy(
                "There is currently another in-progress request using this Stripe-Idempotency-Key"
                    .into(),
            ))
        };
        if self
            .busy_opens
            .try_update(SeqCst, SeqCst, |left| left.checked_sub(1))
            .is_ok()
        {
            return Err(in_use());
        }
        let key = payment.idempotency_key.clone();
        if !self.opening.lock().unwrap().insert(key.clone()) {
            return Err(in_use());
        }
        let delay = self.open_delay_ms.load(SeqCst);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let open = {
            let mut opened = self.opened.lock().unwrap();
            let count = opened.len();
            let entry = opened.entry(key.clone()).or_insert_with(|| {
                (
                    OpenPayment {
                        payment_id: format!("pi_{count}"),
                        client_secret: format!("pi_{count}_secret"),
                    },
                    payment,
                )
            });
            entry.0.clone()
        };
        self.opening.lock().unwrap().remove(&key);
        Ok(open)
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
        let (open, new) = (open.clone(), new.clone());
        drop(opened);
        let state = self.state_of(&open.payment_id);
        Ok(PaymentStatus {
            payment_id: open.payment_id.clone(),
            state,
            amount_capturable: if state == PaymentState::Authorized {
                new.amount_cents
            } else {
                0
            },
            amount_received: if state == PaymentState::Captured {
                new.amount_cents
            } else {
                0
            },
            currency: new.currency.to_ascii_lowercase(),
            capture_before: None,
            lapsed: self.is(&self.lapsed, &open.payment_id),
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
        use std::sync::atomic::Ordering::SeqCst;
        self.refund_attempts.fetch_add(1, SeqCst);
        self.reachable()?;
        if self.refuses_refund.load(SeqCst) {
            return Err(anyhow::Error::new(Refused(
                "This charge is disputed.".into(),
            )));
        }
        let mut refunds = self.refunds.lock().unwrap();
        refunds.push((payment_id.to_owned(), amount_cents));
        Ok(format!("re_{}", refunds.len()))
    }

    async fn refunds_of(
        &self,
        _secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<Vec<RefundRecord>> {
        Ok(self
            .refunds
            .lock()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, (paid, _))| paid == payment_id)
            .map(|(index, (_, amount_cents))| RefundRecord {
                refund_id: format!("re_{}", index + 1),
                amount_cents: *amount_cents,
                succeeded: true,
            })
            .collect())
    }

    async fn cancel_payment(
        &self,
        _secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<Cancellation> {
        match self.state_of(payment_id) {
            PaymentState::Captured => return Ok(Cancellation::Paid),
            PaymentState::Processing => return Ok(Cancellation::Confirming),
            PaymentState::Authorized => return Ok(Cancellation::Authorized),
            PaymentState::Cancelled => return Ok(Cancellation::Cancelled),
            PaymentState::Open => {}
        }
        self.cancelled.lock().unwrap().push(payment_id.to_owned());
        Ok(Cancellation::Cancelled)
    }

    async fn capture_payment(
        &self,
        _secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<CaptureAnswer> {
        self.reachable()?;
        let amount_cents = self.amount_of(payment_id);
        Ok(match self.state_of(payment_id) {
            PaymentState::Captured => CaptureAnswer::Captured { amount_cents },
            PaymentState::Cancelled => CaptureAnswer::Cancelled {
                lapsed: self.is(&self.lapsed, payment_id),
            },
            PaymentState::Authorized
                if !self
                    .refuses_capture
                    .load(std::sync::atomic::Ordering::SeqCst) =>
            {
                self.captured.lock().unwrap().push(payment_id.to_owned());
                CaptureAnswer::Captured { amount_cents }
            }
            PaymentState::Authorized => CaptureAnswer::Refused("Your card was declined.".into()),
            _ => CaptureAnswer::Refused("This payment was not confirmed.".into()),
        })
    }

    async fn release_payment(
        &self,
        _secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<ReleaseAnswer> {
        self.reachable()?;
        Ok(match self.state_of(payment_id) {
            PaymentState::Captured => ReleaseAnswer::Captured {
                amount_cents: self.amount_of(payment_id),
            },
            PaymentState::Cancelled => ReleaseAnswer::Released,
            _ => {
                self.cancelled.lock().unwrap().push(payment_id.to_owned());
                ReleaseAnswer::Released
            }
        })
    }
}

/// A supplier, as far as these tests need it: each read does what a test set.
#[derive(Default)]
struct FakeGoods {
    /// What the next read does, once.
    next_read: Mutex<Option<GoodsMove>>,
    /// The products of each read, in order.
    reads: Mutex<Vec<Vec<String>>>,
    /// The store the reads write to, once the harness has one.
    store: Mutex<Option<cluster::Executor>>,
    /// Who buys the goods once the order is placed: the money then waits.
    supplier: Mutex<Option<String>>,
}

/// What a read finds.
enum GoodsMove {
    /// The product now costs this much.
    Price(String, u64),
    /// The supplier has none left of a product it now counts.
    SoldOut(String),
}

#[async_trait::async_trait]
impl SuppliedGoods for FakeGoods {
    async fn read_again(&self, tenant_id: &str, product_ids: &[String]) {
        self.reads.lock().unwrap().push(product_ids.to_vec());
        let found = self.next_read.lock().unwrap().take();
        let store = self.store.lock().unwrap().clone().unwrap();
        let catalog = timada_catalog::Command::new(store, tenant_id);
        match found {
            Some(GoodsMove::Price(product, cents)) => catalog
                .price_product("supplier", &product, cents, "EUR")
                .await
                .unwrap(),
            Some(GoodsMove::SoldOut(product)) => catalog
                .set_stock("supplier", &product, None, 0)
                .await
                .unwrap(),
            None => {}
        }
    }

    async fn supplier_of(
        &self,
        _tenant_id: &str,
        _lines: &[OrderLine],
    ) -> anyhow::Result<Option<String>> {
        Ok(self.supplier.lock().unwrap().clone())
    }
}

/// VIES, as far as these tests need it: numbers starting `DE1` exist.
struct FakeVies;

#[async_trait::async_trait]
impl vies::Vies for FakeVies {
    async fn check(&self, vat_number: &str, requester: Option<&str>) -> vies::ViesAnswer {
        assert_eq!(requester, Some("FR85120027016"), "the seller asks");
        if vat_number.starts_with("DE1") {
            vies::ViesAnswer::Valid {
                reference: Some(format!("WAPI-{vat_number}")),
            }
        } else {
            vies::ViesAnswer::Invalid
        }
    }
}

/// 1 € = 1.25 $ on the day of the tests, the day before's rate for pounds,
/// and no other currency (no francs): the Bank, as far as these tests need
/// it.
fn fake_rates() -> Arc<dyn ecb::EuroRates> {
    let today = timada_core::civil::format_date(now() as u64);
    let yesterday = timada_core::civil::format_date(now() as u64 - 86_400);
    let mut table = timada_invoicing::rates::RateTable::default();
    table.insert(&today, "USD", "1.25");
    table.insert(&yesterday, "USD", "1.20");
    table.insert(&yesterday, "GBP", "0.80");
    Arc::new(table)
}

/// One node with its platform store and tenants, paying through `G`: the
/// fake Stripe unless said otherwise.
struct Harness<G = FakeStripe> {
    cluster: Arc<Cluster>,
    subscriptions: Vec<evento::subscription::Subscription>,
    platform: Command<cluster::Executor>,
    payments: Arc<Payments>,
    stripe: Arc<G>,
    goods: Arc<FakeGoods>,
    /// What [`Harness::connect_stripe`] saves as the tenant's secret key.
    secret_key: String,
    /// The read models in the store's database.
    db: db::Db,
    dir: PathBuf,
}

async fn harness() -> Harness {
    harness_with(Arc::new(FakeStripe::default()), "sk_test_good").await
}

/// A harness whose tenants pay through `stripe` with `secret_key`.
async fn harness_with<G: PaymentGateway + 'static>(stripe: Arc<G>, secret_key: &str) -> Harness<G> {
    harness_on(stripe, secret_key, true).await
}

/// A harness whose node answers no request to pay: as if the node that
/// took one on stopped.
async fn harness_without_answers() -> Harness {
    harness_on(Arc::new(FakeStripe::default()), "sk_test_good", false).await
}

/// `answers`: whether this node runs the subscription answering requests
/// to pay.
async fn harness_on<G: PaymentGateway + 'static>(
    stripe: Arc<G>,
    secret_key: &str,
    answers: bool,
) -> Harness<G> {
    let dir = std::env::temp_dir().join(format!("timada-payments-{}", ulid::Ulid::generate()));
    std::fs::create_dir_all(&dir).unwrap();
    // The store in memory (a file would fsync every commit of the thirty-odd
    // tests running at once), the read models in it.
    let files = cluster::Files {
        path: PathBuf::from(":memory:"),
    };
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    db::migrate_store_read_models(&store).await.unwrap();
    let store_db = db::Db::from(store.clone());
    let (cluster, platform) = Cluster::open(None, None, &files, store).await.unwrap();
    let payments = Arc::new(Payments::new(
        Some(seal::Sealer::new("payments", SEAL_KEY)),
        stripe.clone(),
        platform.clone(),
    ));
    let subscribed = payments.clone();
    let invoicing_platform = platform.clone();
    let goods = Arc::new(FakeGoods::default());
    *goods.store.lock().unwrap() = Some(platform.clone());
    // The subscriptions the dashboard runs on the store, each following
    // every tenant.
    let mut subscriptions = Vec::new();
    if answers {
        let answering = payments::subscription(payments::Checkouts::new(
            subscribed.clone(),
            fake_rates(),
            goods.clone(),
            platform.clone(),
            "node-0",
        ));
        subscriptions.push(answering.start(&platform).await.unwrap());
    }
    for builder in [
        payments::order_payments_subscription(subscribed.clone(), platform.clone()),
        timada_orders::discount_usage_subscription(platform.clone()),
        timada_orders::stock_usage_subscription(platform.clone()),
        invoicing::subscription(
            invoicing_platform.clone(),
            Arc::new(FakeVies),
            fake_rates(),
            platform.clone(),
        ),
    ] {
        subscriptions.push(builder.start(&platform).await.unwrap());
    }
    Harness {
        cluster,
        subscriptions,
        platform: Command(platform),
        payments,
        stripe,
        goods,
        secret_key: secret_key.to_owned(),
        db: store_db,
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
            region: None,
            country: "FR".into(),
            phone: Some("06 12 34 56 78".into()),
            ..Default::default()
        },
        note: None,
        expected_total_cents,
        vat_number: None,
        billing: None,
        shipping_method: None,
        language: None,
        newsletter: false,
        time_zone: None,
    }
}

impl<G: PaymentGateway + 'static> Harness<G> {
    async fn create(&self, slug: &str) -> TenantState {
        let tenant = self
            .platform
            .create_tenant(CreateTenant {
                name: slug.into(),
                owner_user_id: "alice".into(),
            })
            .await
            .unwrap();
        read_model_subscription::<cluster::Executor>(self.db.clone())
            .run_once(&self.platform.0)
            .await
            .unwrap();
        tenant
    }

    /// Makes the tenant sell as Acme SAS (registered on first use), which a
    /// sale needs.
    async fn set_legal(&self, tenant: &TenantState) {
        let identity = LegalIdentity {
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
        };
        let company_id = match self.platform.create_company("alice", identity).await {
            Ok(company) => company.id,
            Err(_) => {
                self.platform
                    .load_company_siren("120027016")
                    .await
                    .unwrap()
                    .unwrap()
                    .company_id
            }
        };
        if self
            .platform
            .link_tenant("alice", &company_id, &tenant.id, None)
            .await
            .is_err()
        {
            let code: String = tenant
                .slug
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(10)
                .collect();
            self.platform
                .link_tenant("alice", &company_id, &tenant.id, Some(&code))
                .await
                .unwrap();
        }
    }

    /// Links the tenant to its company too.
    async fn connect_stripe(&self, tenant: &TenantState) -> TenantState {
        self.set_legal(tenant).await;
        self.platform
            .configure_payments(
                "alice",
                &tenant.id,
                ConfigurePayments {
                    provider: "stripe".into(),
                    publishable_key: "pk_test_123".into(),
                    secret_key_sealed: self.payments.seal(&tenant.id, &self.secret_key).unwrap(),
                    webhook_secret_sealed: self.payments.seal(&tenant.id, WEBHOOK_SECRET).unwrap(),
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

    /// The store, as the tenant's gateways are built over it.
    fn store(&self, _tenant: &TenantState) -> cluster::Executor {
        self.platform.0.clone()
    }

    /// A cart holding two mugs at 12.50 €; returns the cart and the mug.
    async fn cart(&self, tenant: &TenantState) -> (String, String) {
        let store = self.store(tenant);
        let catalog = timada_catalog::Command::new(store.clone(), &tenant.id);
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
        let orders = timada_orders::Command::new(store, &tenant.id);
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
        self.ask_with(tenant, cart_id, checkout(expected)).await
    }

    async fn ask_with(
        &self,
        tenant: &TenantState,
        cart_id: &str,
        checkout: Checkout,
    ) -> PaymentAnswer {
        let orders = timada_orders::Command::new(self.store(tenant), &tenant.id);
        let request_id = orders
            .request_checkout(STOREFRONT_ACTOR, cart_id, checkout)
            .await
            .unwrap();
        // Generous: under load (another build on the machine) the
        // subscriptions answer in seconds, not milliseconds.
        tokio::time::timeout(Duration::from_secs(30), async {
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
        self.try_webhook(tenant, body, header).await.unwrap()
    }

    /// The same, an error kept: Stripe is told to try again.
    async fn try_webhook(
        &self,
        tenant: &TenantState,
        body: &str,
        header: Option<String>,
    ) -> anyhow::Result<Outcome> {
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
            self.store(&tenant),
            body.as_bytes(),
            header.as_deref(),
            now(),
        )
        .await
    }

    /// Waits for the money of the order to be taken; the order then.
    async fn paid(&self, tenant: &TenantState, order_id: &str) -> timada_orders::OrderState {
        let orders = timada_orders::Command::new(self.store(tenant), &tenant.id);
        eventually(async || {
            orders
                .load_order(order_id)
                .await
                .unwrap()
                .filter(|order| order.is_paid())
        })
        .await
    }

    /// The tenant's Stripe account, opened.
    async fn account(&self, tenant: &TenantState) -> payments::Account {
        let tenant = self
            .platform
            .load_tenant(&tenant.id)
            .await
            .unwrap()
            .unwrap();
        self.payments.account_of(&tenant).unwrap().unwrap()
    }

    async fn finish(self) {
        for subscription in &self.subscriptions {
            subscription.stop();
        }
        for subscription in &self.subscriptions {
            subscription.stopped().await;
        }
        self.cluster.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Harness {
    /// Has the shopper confirm a cart shipped as `checkout` says, end to
    /// end; returns the order, its money held.
    async fn place(&self, tenant: &TenantState, checkout: Checkout) -> String {
        let (cart_id, _) = self.cart(tenant).await;
        let PaymentAnswer::Requested(payment) = self.ask_with(tenant, &cart_id, checkout).await
        else {
            panic!("a payment is opened");
        };
        self.stripe.authorize(&payment.payment_id);
        let body = authorized(&payment.payment_id);
        let Outcome::Placed { order_id } = self.webhook(tenant, &body, signed(&body)).await else {
            panic!("the order is placed");
        };
        order_id
    }

    /// The same, for goods the shop holds: returns the order once its money
    /// is taken.
    async fn buy(&self, tenant: &TenantState, checkout: Checkout) -> String {
        let order_id = self.place(tenant, checkout).await;
        self.paid(tenant, &order_id).await;
        order_id
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// What Stripe sends about a payment: `kind` is the event type.
fn payment_event(kind: &str, payment_id: &str) -> String {
    format!(
        r#"{{"id":"evt_1","type":"{kind}","data":{{"object":{{"id":"{payment_id}","object":"payment_intent"}}}}}}"#
    )
}

/// The shopper confirmed the payment: its money is held.
fn authorized(payment_id: &str) -> String {
    payment_event("payment_intent.amount_capturable_updated", payment_id)
}

fn succeeded(payment_id: &str) -> String {
    payment_event("payment_intent.succeeded", payment_id)
}

fn canceled(payment_id: &str) -> String {
    payment_event("payment_intent.canceled", payment_id)
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

    // Not confirmed yet: Stripe's word is that nothing is held.
    let body = authorized(&payment.payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Ignored
    );
    h.stripe.authorize(&payment.payment_id);
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
    // The shop holds the mugs: their money is taken at once, once.
    let order = h.paid(&acme, &order_id).await;
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(order.paid_cents, 2500);
    assert_eq!(order.supplier, None);
    assert_eq!(
        *h.stripe.captured.lock().unwrap(),
        vec![payment.payment_id.clone()]
    );
    // Stripe says so too: known already.
    let body = succeeded(&payment.payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Paid {
            order_id: order_id.clone()
        }
    );
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 2500);
    assert_eq!(order.payment.payment_id, payment.payment_id);
    assert_eq!(order.payment.currency, "EUR");
    assert_eq!(order.email, "ada@example.org");

    // The order is invoiced, at the merchant's default rate.
    let invoicing = timada_invoicing::Command::new(h.store(&acme), &acme.id);
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

/// Waits for what a subscription writes; generously, for a loaded machine.
async fn eventually<T>(mut probe: impl AsyncFnMut() -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), async {
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
    h.stripe.authorize(&payment.payment_id);
    let body = authorized(&payment.payment_id);

    assert_eq!(h.webhook(&acme, &body, None).await, Outcome::Rejected);
    let forged = Some(signature::sign(body.as_bytes(), "whsec_guess", now()));
    assert_eq!(h.webhook(&acme, &body, forged).await, Outcome::Rejected);
    let stale = Some(signature::sign(
        body.as_bytes(),
        WEBHOOK_SECRET,
        now() - 3600,
    ));
    assert_eq!(h.webhook(&acme, &body, stale).await, Outcome::Rejected);
    // A charge of a payment nobody refunded, and one without its payment.
    let other = format!(
        r#"{{"id":"evt_2","type":"charge.refunded","data":{{"object":{{"id":"ch_1","payment_intent":"{}"}}}}}}"#,
        payment.payment_id
    );
    assert_eq!(
        h.webhook(&acme, &other, signed(&other)).await,
        Outcome::Ignored,
        "no order yet: nothing to reconcile"
    );
    let bare = r#"{"id":"evt_3","type":"charge.refunded","data":{"object":{"id":"ch_1"}}}"#;
    assert_eq!(h.webhook(&acme, bare, signed(bare)).await, Outcome::Ignored);

    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
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
    timada_catalog::Command::new(h.store(&acme), &acme.id)
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
async fn a_price_the_supplier_moved_is_found_before_the_payment_and_confirmed_at_the_new_total() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, mug) = h.cart(&acme).await;

    // The shopper saw 25.00; the read before the payment finds 13.00 each.
    *h.goods.next_read.lock().unwrap() = Some(GoodsMove::Price(mug.clone(), 1300));
    assert_eq!(
        h.ask(&acme, &cart_id, Some(2500)).await,
        PaymentAnswer::Refused("price_changed".into())
    );
    assert_eq!(h.stripe.opened_count(), 0, "nothing was opened with Stripe");
    assert_eq!(
        h.goods.reads.lock().unwrap().as_slice(),
        [vec![mug.clone()]]
    );

    // Confirmed at the new total, it is charged.
    let PaymentAnswer::Requested(payment) = h.ask(&acme, &cart_id, Some(2600)).await else {
        panic!("a payment is opened");
    };
    assert_eq!(payment.total_cents, 2600);
    assert_eq!(h.goods.reads.lock().unwrap().len(), 2);
    h.finish().await;
}

#[tokio::test]
async fn goods_the_supplier_ran_out_of_are_found_before_the_payment() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, mug) = h.cart(&acme).await;
    timada_catalog::Command::new(h.store(&acme), &acme.id)
        .set_stock("alice", &mug, None, 10)
        .await
        .unwrap();

    *h.goods.next_read.lock().unwrap() = Some(GoodsMove::SoldOut(mug.clone()));
    assert_eq!(
        h.ask(&acme, &cart_id, Some(2500)).await,
        PaymentAnswer::Refused("out_of_stock".into())
    );
    assert_eq!(h.stripe.opened_count(), 0, "nothing was opened with Stripe");
    h.finish().await;
}

#[tokio::test]
async fn a_read_that_finds_nothing_lets_the_payment_go_at_the_price_the_shopper_saw() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, _) = h.cart(&acme).await;

    assert!(matches!(
        h.ask(&acme, &cart_id, Some(2500)).await,
        PaymentAnswer::Requested(_)
    ));
    assert_eq!(h.goods.reads.lock().unwrap().len(), 1);
    h.finish().await;
}

/// What the goods of the cart hold, and what is left for anyone else.
async fn stock_of(
    h: &Harness,
    tenant: &TenantState,
    product_id: &str,
) -> timada_catalog::StockState {
    timada_catalog::load_stock(&h.store(tenant), &tenant.id, product_id, None)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn nodes_answering_one_request_at_once_hold_and_open_once() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, mug) = h.cart(&acme).await;
    timada_catalog::Command::new(h.store(&acme), &acme.id)
        .set_stock("alice", &mug, None, 10)
        .await
        .unwrap();
    // Stripe takes its time, and answers 409 to a second call with the key
    // of a payment it is still opening.
    h.stripe
        .open_delay_ms
        .store(300, std::sync::atomic::Ordering::SeqCst);

    // This node's subscription and two other nodes answer the same request.
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let request_id = orders
        .request_checkout(STOREFRONT_ACTOR, &cart_id, checkout(Some(2500)))
        .await
        .unwrap();
    let node = |name: &str| {
        Checkouts::new(
            h.payments.clone(),
            fake_rates(),
            h.goods.clone(),
            h.store(&acme),
            name,
        )
    };
    let (one, two) = (node("node-1"), node("node-2"));
    let asked_at = now() as u64;
    let (first, second) = tokio::join!(
        one.answer(&acme.id, &cart_id, &request_id, asked_at),
        two.answer(&acme.id, &cart_id, &request_id, asked_at),
    );
    first.unwrap();
    second.unwrap();

    let answer = eventually(async || {
        let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
        cart.request(&request_id).unwrap().answer.clone()
    })
    .await;
    assert!(matches!(answer, PaymentAnswer::Requested(_)), "{answer:?}");
    assert_eq!(
        h.stripe
            .open_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one node asked Stripe"
    );
    // The goods stay held for the payment.
    let stock = stock_of(&h, &acme, &mug).await;
    assert!(stock.holds.contains_key(&cart_id), "{stock:?}");
    assert_eq!(stock.available_at(now() as u64), Some(8));
    h.finish().await;
}

#[tokio::test]
async fn a_payment_stripe_is_still_opening_is_asked_for_again() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, mug) = h.cart(&acme).await;
    timada_catalog::Command::new(h.store(&acme), &acme.id)
        .set_stock("alice", &mug, None, 10)
        .await
        .unwrap();
    // Stripe answers 409 once: a request with the same key is under way.
    h.stripe
        .busy_opens
        .store(1, std::sync::atomic::Ordering::SeqCst);

    let answer = h.ask(&acme, &cart_id, Some(2500)).await;
    assert!(
        matches!(answer, PaymentAnswer::Requested(_)),
        "not refused: {answer:?}"
    );
    assert_eq!(
        h.stripe
            .open_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "asked again"
    );
    let stock = stock_of(&h, &acme, &mug).await;
    assert!(stock.holds.contains_key(&cart_id), "{stock:?}");
    assert_eq!(stock.available_at(now() as u64), Some(8));
    h.finish().await;
}

/// Holds the two mugs of the cart for its checkout, as the node that took
/// a request on does before it asks Stripe.
async fn hold_the_mugs(h: &Harness, tenant: &TenantState, cart_id: &str, mug: &str) {
    timada_catalog::Command::new(h.store(tenant), &tenant.id)
        .reserve_stock(
            DASHBOARD_ACTOR,
            cart_id,
            &[StockLine {
                product_id: mug.to_owned(),
                variant_id: None,
                quantity: 2,
            }],
            RESERVATION_SECS,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_request_whose_node_stopped_is_refused_once_its_claim_lapses() {
    let h = harness_without_answers().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, mug) = h.cart(&acme).await;
    timada_catalog::Command::new(h.store(&acme), &acme.id)
        .set_stock("alice", &mug, None, 10)
        .await
        .unwrap();
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let sweep = UnansweredCheckouts::new(h.db.clone(), h.store(&acme));

    // node-9 takes the request on, holds the mugs, and stops for good.
    let request_id = orders
        .request_checkout(STOREFRONT_ACTOR, &cart_id, checkout(Some(2500)))
        .await
        .unwrap();
    orders
        .claim_checkout(DASHBOARD_ACTOR, &cart_id, &request_id, "node-9")
        .await
        .unwrap();
    hold_the_mugs(&h, &acme, &cart_id, &mug).await;
    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();

    // While its claim holds, the request is node-9's.
    let now = now() as u64;
    assert_eq!(sweep.sweep(now).await.unwrap(), 0);
    assert_eq!(
        sweep
            .sweep(now + timada_orders::CHECKOUT_CLAIM_SECS - 60)
            .await
            .unwrap(),
        0
    );
    // Once it lapsed, the request is refused and the mugs go back.
    let lapsed = now + timada_orders::CHECKOUT_CLAIM_SECS + 1;
    assert_eq!(sweep.sweep(lapsed).await.unwrap(), 1);
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.request(&request_id).unwrap().answer,
        Some(PaymentAnswer::Refused("payments_unavailable".into()))
    );
    let stock = stock_of(&h, &acme, &mug).await;
    assert!(!stock.holds.contains_key(&cart_id), "{stock:?}");
    assert_eq!(stock.available_at(now), Some(10));
    // Answered: nothing more to sweep.
    assert_eq!(sweep.sweep(lapsed).await.unwrap(), 0);
    h.finish().await;
}

#[tokio::test]
async fn a_refusal_that_is_not_the_latest_request_gives_nothing_back() {
    let h = harness_without_answers().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, mug) = h.cart(&acme).await;
    timada_catalog::Command::new(h.store(&acme), &acme.id)
        .set_stock("alice", &mug, None, 10)
        .await
        .unwrap();
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let sweep = UnansweredCheckouts::new(h.db.clone(), h.store(&acme));

    // node-9 takes the first request on and stops; the shopper asks again,
    // and another node holds the mugs for the second and opens its payment.
    let first = orders
        .request_checkout(STOREFRONT_ACTOR, &cart_id, checkout(Some(2500)))
        .await
        .unwrap();
    orders
        .claim_checkout(DASHBOARD_ACTOR, &cart_id, &first, "node-9")
        .await
        .unwrap();
    let second = orders
        .request_checkout(STOREFRONT_ACTOR, &cart_id, checkout(Some(2500)))
        .await
        .unwrap();
    hold_the_mugs(&h, &acme, &cart_id, &mug).await;
    let pending = orders
        .pending_payment(&cart_id, &second)
        .await
        .unwrap()
        .unwrap();
    orders
        .record_payment_request(
            DASHBOARD_ACTOR,
            &cart_id,
            pending,
            timada_orders::OpenedPayment {
                provider: timada_orders::PROVIDER_STRIPE.into(),
                payment_id: "pi_second".into(),
                client_secret: "pi_second_secret".into(),
                publishable_key: "pk_test_123".into(),
            },
        )
        .await
        .unwrap();
    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();

    // The first is refused; the mugs stay held for the second's payment.
    let now = now() as u64;
    assert_eq!(
        sweep
            .sweep(now + timada_orders::CHECKOUT_CLAIM_SECS + 1)
            .await
            .unwrap(),
        1
    );
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.request(&first).unwrap().answer,
        Some(PaymentAnswer::Refused("payments_unavailable".into()))
    );
    assert!(cart.request(&second).unwrap().payment().is_some());
    let stock = stock_of(&h, &acme, &mug).await;
    assert!(stock.holds.contains_key(&cart_id), "{stock:?}");
    assert_eq!(stock.available_at(now), Some(8));
    h.finish().await;
}

/// A VAT-registered merchant without the One-Stop Shop sells to consumers
/// elsewhere in the EU in euros or in a currency the Bank publishes a euro
/// rate for, which the sale is counted at; with the One-Stop Shop, in any
/// currency.
#[tokio::test]
async fn a_distance_sale_in_another_currency_counts_at_the_euro_rate() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let store = h.store(&acme);
    let catalog = timada_catalog::Command::new(store.clone(), &acme.id);
    let orders = timada_orders::Command::new(store.clone(), &acme.id);
    let invoicing = timada_invoicing::Command::new(store.clone(), &acme.id);
    let priced_in = async |name: &str, currency: &str| {
        let product = catalog
            .create_product(
                "alice",
                CreateProduct {
                    name: name.into(),
                    slug: None,
                },
            )
            .await
            .unwrap();
        catalog
            .price_product("alice", &product.id, 3000, currency)
            .await
            .unwrap();
        catalog.publish_product("alice", &product.id).await.unwrap();
        product.id
    };
    let hat = priced_in("Hat", "USD").await;
    let scarf = priced_in("Scarf", "CHF").await;
    let cart_of = async |product_id: &str| {
        let cart_id = orders.create_cart(STOREFRONT_ACTOR).await.unwrap();
        orders
            .add_to_cart(
                STOREFRONT_ACTOR,
                &cart_id,
                CartItem {
                    product_id: product_id.to_owned(),
                    variant_id: None,
                    quantity: 1,
                },
            )
            .await
            .unwrap();
        cart_id
    };
    let to_germany = Checkout {
        shipping: ShippingAddress {
            name: "Ada".into(),
            line1: "Unter den Linden 1".into(),
            postal_code: "10117".into(),
            city: "Berlin".into(),
            region: None,
            country: "DE".into(),
            phone: Some("+49 30 123456".into()),
            ..Default::default()
        },
        ..checkout(Some(3000))
    };

    // Dollars: the sale goes ahead, under the threshold it carries French
    // VAT, and the invoice says what it counted for and its VAT in euros.
    // 30.00 $ at 20 %: 25.00 $, 20.00 € at 1.25; 5.00 $ of VAT, 4.00 €.
    let cart_id = cart_of(&hat).await;
    let PaymentAnswer::Requested(payment) = h.ask_with(&acme, &cart_id, to_germany.clone()).await
    else {
        panic!("a payment is opened");
    };
    h.stripe.authorize(&payment.payment_id);
    let body = authorized(&payment.payment_id);
    let Outcome::Placed { order_id } = h.webhook(&acme, &body, signed(&body)).await else {
        panic!("the order is placed");
    };
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    assert_eq!(invoice.currency, "USD");
    assert_eq!(
        invoice.regime,
        timada_invoicing::tax::Regime::DistanceOrigin
    );
    assert_eq!(invoice.vat[0].rate_bps, 2000);
    assert_eq!(invoice.eu_distance_sales_cents, 2000);
    let rate = invoice
        .euro_rate
        .clone()
        .expect("the day's rate is recorded");
    assert_eq!(rate.currency, "USD");
    assert_eq!(rate.per_euro, "1.25");
    assert_eq!(rate.source, timada_invoicing::rates::ECB_SOURCE);
    assert_eq!(invoice.total_vat_eur_cents, Some(400));

    // Francs, which the Bank of these tests publishes no euro rate for:
    // refused, the sale would owe German VAT the merchant cannot remit.
    let cart_id = cart_of(&scarf).await;
    assert_eq!(
        h.ask_with(&acme, &cart_id, to_germany.clone()).await,
        PaymentAnswer::Refused("oss_required_for_currency".into())
    );
    // To France, or to a business, the currency is no concern.
    let cart_id = cart_of(&scarf).await;
    assert!(matches!(
        h.ask(&acme, &cart_id, Some(3000)).await,
        PaymentAnswer::Requested(_)
    ));
    orders.set_business_buyers("alice", true).await.unwrap();
    let cart_id = cart_of(&scarf).await;
    let business = Checkout {
        vat_number: Some("DE123456789".into()),
        ..to_germany.clone()
    };
    assert!(matches!(
        h.ask_with(&acme, &cart_id, business).await,
        PaymentAnswer::Requested(_)
    ));

    // Registered for the One-Stop Shop: the sale goes ahead.
    let company_id = h
        .platform
        .load_company_siren("120027016")
        .await
        .unwrap()
        .unwrap()
        .company_id;
    let company = h.platform.load_company(&company_id).await.unwrap().unwrap();
    let identity = LegalIdentity {
        oss_registered: true,
        ..company.identity.unwrap()
    };
    h.platform
        .set_company_identity("alice", &company_id, identity)
        .await
        .unwrap();
    let cart_id = cart_of(&scarf).await;
    let PaymentAnswer::Requested(payment) = h.ask_with(&acme, &cart_id, to_germany).await else {
        panic!("a payment is opened");
    };
    assert_eq!(payment.lines[0].tax_category(), "standard");
    h.finish().await;
}

/// A refund made in Stripe's own dashboard, or one Stripe took whose
/// recording failed here, reaches the order through `charge.refunded`.
#[tokio::test]
async fn a_refund_made_in_stripe_is_recorded_from_its_webhook() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let order_id = h.buy(&acme, checkout(Some(2500))).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    let payment_id = order.payment.payment_id.clone();

    // Refunded by hand in Stripe: the order knows nothing of it.
    h.stripe
        .refunds
        .lock()
        .unwrap()
        .push((payment_id.clone(), 1000));
    let body = format!(
        r#"{{"id":"evt_9","type":"charge.refunded","data":{{"object":{{"id":"ch_9","payment_intent":"{payment_id}"}}}}}}"#
    );
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Reconciled { recorded: 1 }
    );
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.refunded_cents, 1000);
    assert_eq!(order.refund_ids, vec!["re_1".to_owned()]);
    // Told again: known already.
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Reconciled { recorded: 0 }
    );
    // And the credit note follows, as for a refund made here.
    let invoicing = timada_invoicing::Command::new(h.store(&acme), &acme.id);
    let credit =
        eventually(async || invoicing.load_credit_note_of_refund("re_1").await.unwrap()).await;
    assert_eq!(credit.total_cents, 1000);
    h.finish().await;
}

/// A refund made here raises `charge.refunded` too, which may arrive before
/// the refund is recorded or after: it counts once either way, with one
/// credit note.
#[tokio::test]
async fn a_refund_made_here_counts_once_whenever_its_webhook_arrives() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let invoicing = timada_invoicing::Command::new(h.store(&acme), &acme.id);

    for webhook_first in [true, false] {
        let order_id = h.buy(&acme, checkout(Some(2500))).await;
        let order = orders.load_order(&order_id).await.unwrap().unwrap();
        let pending = orders
            .request_refund("alice", &order_id, order.total_cents)
            .await
            .unwrap();
        let refund_id = h
            .stripe
            .refund(
                "sk_test_good",
                &pending.payment_id,
                pending.amount_cents,
                &pending.idempotency_key,
            )
            .await
            .unwrap();
        let body = format!(
            r#"{{"id":"evt_9","type":"charge.refunded","data":{{"object":{{"id":"ch_9","payment_intent":"{}"}}}}}}"#,
            pending.payment_id
        );

        if webhook_first {
            assert_eq!(
                h.webhook(&acme, &body, signed(&body)).await,
                Outcome::Reconciled { recorded: 1 }
            );
        }
        orders
            .record_refund("alice", &order_id, &refund_id, pending.amount_cents)
            .await
            .unwrap();
        if !webhook_first {
            assert_eq!(
                h.webhook(&acme, &body, signed(&body)).await,
                Outcome::Reconciled { recorded: 0 }
            );
        }
        orders
            .request_cancellation("alice", &order_id, "Out of stock".to_owned(), false)
            .await
            .unwrap();
        orders.complete_cancellation(&order_id).await.unwrap();

        let order = orders.load_order(&order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::Cancelled);
        assert_eq!(order.refunded_cents, order.total_cents);
        assert_eq!(order.refund_ids, vec![refund_id.clone()]);
        let credit = eventually(async || {
            invoicing
                .load_credit_note_of_refund(&refund_id)
                .await
                .unwrap()
        })
        .await;
        assert_eq!(credit.total_cents, order.total_cents);
    }
    h.finish().await;
}

/// The holds a checkout takes last thirty minutes; a payment confirmed
/// after that finds them lapsed. The test gives them back by hand, as time
/// would, then lets the late payment in.
#[tokio::test]
async fn a_late_payment_places_the_order_or_gives_its_hold_back() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let store = h.store(&acme);
    let catalog = timada_catalog::Command::new(store.clone(), &acme.id);
    let orders = timada_orders::Command::new(store.clone(), &acme.id);
    let discounts = timada_discounts::Command::new(store.clone(), &acme.id);
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
    // Two on the shelf.
    catalog.set_stock("alice", &mug.id, None, 2).await.unwrap();
    let once = async |code: &str| {
        discounts
            .create_discount(
                "alice",
                timada_discounts::CreateDiscount {
                    code: code.into(),
                    value: timada_discounts::DiscountValue::Percent(1000),
                    max_uses: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
    };
    let ten = once("TEN").await;
    let lines = |quantity: u32| {
        vec![StockLine {
            product_id: mug.id.clone(),
            variant_id: None,
            quantity,
        }]
    };
    // A cart of one mug, with a code or not, asked to pay: the payment opened.
    let cart_with = async |code: Option<&str>, email: &str| {
        let cart_id = orders.create_cart(STOREFRONT_ACTOR).await.unwrap();
        orders
            .add_to_cart(
                STOREFRONT_ACTOR,
                &cart_id,
                CartItem {
                    product_id: mug.id.clone(),
                    variant_id: None,
                    quantity: 1,
                },
            )
            .await
            .unwrap();
        if let Some(code) = code {
            orders
                .apply_discount(STOREFRONT_ACTOR, &cart_id, code)
                .await
                .unwrap();
        }
        let total = if code.is_some() { 1125 } else { 1250 };
        let checkout = Checkout {
            email: email.into(),
            ..checkout(Some(total))
        };
        let PaymentAnswer::Requested(payment) = h.ask_with(&acme, &cart_id, checkout).await else {
            panic!("a payment is opened");
        };
        (cart_id, *payment)
    };
    let pay = async |payment_id: &str| {
        h.stripe.authorize(payment_id);
        let body = authorized(payment_id);
        h.webhook(&acme, &body, signed(&body)).await
    };
    let stock_consumed_by = async |cart_id: &str| {
        eventually(async || {
            catalog
                .load_stock(&mug.id, None)
                .await
                .unwrap()
                .filter(|stock| stock.consumed_by_cart(cart_id))
        })
        .await
    };

    // Ada holds a mug and the code; her holds lapse. Nobody takes them in
    // between: her late payment takes them again and places the order.
    let (ada_cart, ada_payment) = cart_with(Some("TEN"), "ada@example.org").await;
    catalog
        .release_stock(DASHBOARD_ACTOR, &ada_cart, &lines(1))
        .await
        .unwrap();
    discounts
        .release_use(DASHBOARD_ACTOR, &ten.id, &ada_cart, "ada@example.org")
        .await
        .unwrap();
    assert!(
        catalog
            .load_stock(&mug.id, None)
            .await
            .unwrap()
            .unwrap()
            .holds
            .is_empty()
    );
    let Outcome::Placed { order_id } = pay(&ada_payment.payment_id).await else {
        panic!("placed");
    };
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 1125);
    assert_eq!(stock_consumed_by(&ada_cart).await.on_hand, 1);

    // Bob holds the last mug; his hold lapses and Carol buys it. Bob's
    // payment then finds the shelf empty: its hold goes back, no order,
    // nothing charged.
    let (bob_cart, bob_payment) = cart_with(None, "bob@example.org").await;
    catalog
        .release_stock(DASHBOARD_ACTOR, &bob_cart, &lines(1))
        .await
        .unwrap();
    let (carol_cart, carol_payment) = cart_with(None, "carol@example.org").await;
    let Outcome::Placed { .. } = pay(&carol_payment.payment_id).await else {
        panic!("placed");
    };
    assert_eq!(stock_consumed_by(&carol_cart).await.on_hand, 0);
    let returned = pay(&bob_payment.payment_id).await;
    assert_eq!(
        returned,
        Outcome::Returned {
            code: "out_of_stock".into(),
        }
    );
    assert_eq!(
        *h.stripe.cancelled.lock().unwrap(),
        vec![bob_payment.payment_id.clone()]
    );
    assert!(h.stripe.refunds.lock().unwrap().is_empty(), "nothing taken");
    assert!(
        orders
            .load_order_of_cart(&bob_cart)
            .await
            .unwrap()
            .is_none()
    );
    let cart = orders.load_cart(&bob_cart).await.unwrap().unwrap();
    let recorded = cart.returned_payment(&bob_payment.payment_id).unwrap();
    assert_eq!(recorded.code, "out_of_stock");
    assert!(!cart.is_paying(), "nothing left to confirm");
    assert_eq!(cart.status(), "open");
    // Told twice: the same answer, one release.
    let body = authorized(&bob_payment.payment_id);
    assert_eq!(h.webhook(&acme, &body, signed(&body)).await, returned);
    assert_eq!(h.stripe.cancelled.lock().unwrap().len(), 1);

    // The same with a code good once: Dave holds its use, lets it lapse, Erin
    // spends it. Dave's hold is given back for the code; the shelf (filled
    // again) is left as it was.
    catalog.set_stock("alice", &mug.id, None, 10).await.unwrap();
    let five = once("FIVE").await;
    let (dave_cart, dave_payment) = cart_with(Some("FIVE"), "dave@example.org").await;
    discounts
        .release_use(DASHBOARD_ACTOR, &five.id, &dave_cart, "dave@example.org")
        .await
        .unwrap();
    let (erin_cart, erin_payment) = cart_with(Some("FIVE"), "erin@example.org").await;
    let Outcome::Placed { .. } = pay(&erin_payment.payment_id).await else {
        panic!("placed");
    };
    stock_consumed_by(&erin_cart).await;
    assert_eq!(
        pay(&dave_payment.payment_id).await,
        Outcome::Returned {
            code: "discount_exhausted".into(),
        }
    );
    assert!(
        h.stripe
            .cancelled
            .lock()
            .unwrap()
            .contains(&dave_payment.payment_id)
    );
    assert!(
        orders
            .load_order_of_cart(&dave_cart)
            .await
            .unwrap()
            .is_none()
    );
    let stock = catalog.load_stock(&mug.id, None).await.unwrap().unwrap();
    assert!(
        !stock.holds.contains_key(&dave_cart),
        "a payment given back holds nothing: {:?}",
        stock.holds
    );
    h.finish().await;
}

#[tokio::test]
async fn a_card_form_left_open_past_its_holds_is_closed_before_it_charges() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let sweep = LapsedCheckouts::new(h.db.clone(), h.store(&acme), h.payments.clone());
    let open = async || {
        let (cart_id, _) = h.cart(&acme).await;
        let PaymentAnswer::Requested(payment) = h.ask(&acme, &cart_id, Some(2500)).await else {
            panic!("a payment is opened");
        };
        (cart_id, *payment)
    };
    // Ada walks away from her card form; Bob confirmed a bank debit Stripe
    // is settling; Carol confirmed, her webhook still to come; Dave's was
    // cancelled in Stripe's own dashboard.
    let (ada_cart, ada) = open().await;
    let (bob_cart, bob) = open().await;
    let (carol_cart, carol) = open().await;
    let (dave_cart, dave) = open().await;
    h.stripe
        .confirming
        .lock()
        .unwrap()
        .push(bob.payment_id.clone());
    h.stripe.authorize(&carol.payment_id);
    h.stripe
        .cancelled
        .lock()
        .unwrap()
        .push(dave.payment_id.clone());
    // The carts the sweep looks at come from the read model, a shop at a
    // time and a page at a time.
    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();
    assert_eq!(
        timada_orders::paying_tenants_before(&h.db, i64::MAX)
            .await
            .unwrap(),
        vec![acme.id.clone()]
    );
    let first = timada_orders::paying_carts_before(&h.db, &acme.id, i64::MAX, None, 3)
        .await
        .unwrap();
    assert_eq!(first.len(), 3);
    let rest = timada_orders::paying_carts_before(
        &h.db,
        &acme.id,
        i64::MAX,
        Some(&CartCursor::from(&first[2])),
        3,
    )
    .await
    .unwrap();
    assert_eq!(rest.len(), 1);
    assert!(
        !first.contains(&rest[0]),
        "the page goes on after the cursor"
    );
    let now = now() as u64;

    // The holds stand: nothing is touched.
    assert_eq!(sweep.sweep(now).await.unwrap(), 0);
    assert_eq!(h.stripe.cancelled.lock().unwrap().len(), 1);

    // They lapsed: Ada's payment is cancelled with Stripe and Dave's, found
    // cancelled there, is recorded as such; Bob's is left to the webhook,
    // and Carol's, confirmed, becomes her order as the webhook would make it.
    let later = now + RESERVATION_SECS;
    assert_eq!(sweep.sweep(later).await.unwrap(), 2);
    let carol_order = orders
        .load_order_of_cart(&carol_cart)
        .await
        .unwrap()
        .expect("the sweep placed the order of a confirmed payment");
    assert_eq!(carol_order.payment.payment_id, carol.payment_id);
    assert_eq!(
        *h.stripe.cancelled.lock().unwrap(),
        vec![dave.payment_id.clone(), ada.payment_id.clone()]
    );
    let cart = orders.load_cart(&ada_cart).await.unwrap().unwrap();
    assert_eq!(
        cart.requests[0].answer,
        Some(PaymentAnswer::Cancelled(ada.payment_id.clone()))
    );
    assert!(!cart.is_paying());
    assert_eq!(cart.status(), "open");
    let cart = orders.load_cart(&dave_cart).await.unwrap().unwrap();
    assert!(cart.is_payment_cancelled(&dave.payment_id));
    let cart = orders.load_cart(&bob_cart).await.unwrap().unwrap();
    assert!(cart.is_paying(), "left to the shopper");
    // Looked at again: nothing more, and Stripe is not asked to cancel twice.
    assert_eq!(sweep.sweep(later).await.unwrap(), 0);
    assert_eq!(h.stripe.cancelled.lock().unwrap().len(), 2);

    // Carol's webhook lands: the same order.
    let body = authorized(&carol.payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Placed {
            order_id: carol_order.id.clone()
        }
    );
    // Bob's debit fails and his form is open again: the sweep does not ask
    // Stripe about a payment it left alone until its time comes, then
    // closes it.
    h.stripe.confirming.lock().unwrap().clear();
    assert_eq!(sweep.sweep(later).await.unwrap(), 0);
    assert_eq!(sweep.sweep(later + RECHECK_SECS).await.unwrap(), 1);
    assert!(
        orders
            .load_cart(&bob_cart)
            .await
            .unwrap()
            .unwrap()
            .is_payment_cancelled(&bob.payment_id)
    );
    // Ada comes back and asks to pay again: a new payment opens.
    let PaymentAnswer::Requested(again) = h.ask(&acme, &ada_cart, Some(2500)).await else {
        panic!("a payment is opened");
    };
    assert_ne!(again.payment_id, ada.payment_id);
    h.finish().await;
}

#[tokio::test]
async fn a_second_payment_of_a_cart_is_given_back() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, _) = h.cart(&acme).await;
    // Two tabs: two payments opened, both confirmed.
    let PaymentAnswer::Requested(first) = h.ask(&acme, &cart_id, None).await else {
        panic!("a payment is opened");
    };
    let PaymentAnswer::Requested(second) = h.ask(&acme, &cart_id, None).await else {
        panic!("a payment is opened");
    };
    assert_ne!(first.payment_id, second.payment_id);
    h.stripe.authorize(&first.payment_id);
    h.stripe.authorize(&second.payment_id);

    let body = authorized(&first.payment_id);
    assert!(matches!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Placed { .. }
    ));
    let body = authorized(&second.payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Returned {
            code: "duplicate_payment".into()
        }
    );
    assert_eq!(
        *h.stripe.cancelled.lock().unwrap(),
        vec![second.payment_id.clone()]
    );
    assert!(h.stripe.refunds.lock().unwrap().is_empty(), "nothing taken");
    // The cart says so, and told again, nothing more happens.
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.returned_payment(&second.payment_id)
            .map(|returned| returned.code.as_str()),
        Some("duplicate_payment")
    );
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Returned {
            code: "duplicate_payment".into()
        }
    );
    assert_eq!(h.stripe.cancelled.lock().unwrap().len(), 1);
    h.finish().await;
}

/// A cart of two mugs asked to pay, whose holds then lapsed with the shelf
/// emptied meanwhile: its payment, confirmed now, buys nothing. Stripe does
/// not answer when asked to give it back.
async fn late_payment_stripe_cannot_give_back(
    h: &Harness,
    tenant: &TenantState,
) -> (String, String) {
    let (cart_id, mug) = h.cart(tenant).await;
    let catalog = timada_catalog::Command::new(h.store(tenant), &tenant.id);
    catalog.set_stock("alice", &mug, None, 2).await.unwrap();
    let PaymentAnswer::Requested(payment) = h.ask(tenant, &cart_id, None).await else {
        panic!("a payment is opened");
    };
    catalog
        .release_stock(
            DASHBOARD_ACTOR,
            &cart_id,
            &[StockLine {
                product_id: mug.clone(),
                variant_id: None,
                quantity: 2,
            }],
        )
        .await
        .unwrap();
    catalog.set_stock("alice", &mug, None, 0).await.unwrap();
    h.stripe
        .unreachable
        .store(true, std::sync::atomic::Ordering::SeqCst);
    h.stripe.authorize(&payment.payment_id);
    let body = authorized(&payment.payment_id);
    assert!(
        h.try_webhook(tenant, &body, signed(&body)).await.is_err(),
        "Stripe should be asked again"
    );
    (cart_id, payment.payment_id.clone())
}

#[tokio::test]
async fn a_payment_given_back_is_recorded_before_stripe_is_asked() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let (cart_id, payment_id) = late_payment_stripe_cannot_give_back(&h, &acme).await;

    // The cart says the payment goes back, and why, before Stripe is asked.
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.pending_return(&payment_id).map(|p| p.code.as_str()),
        Some("out_of_stock")
    );
    assert!(h.stripe.cancelled.lock().unwrap().is_empty());

    // Stripe did give the hold back, its answer lost on the way: its word
    // that the hold went records the return, for the reason it was asked.
    h.stripe
        .unreachable
        .store(false, std::sync::atomic::Ordering::SeqCst);
    h.stripe.cancelled.lock().unwrap().push(payment_id.clone());
    let body = canceled(&payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Returned {
            code: "out_of_stock".into()
        }
    );
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.returned_payment(&payment_id).map(|r| r.code.as_str()),
        Some("out_of_stock")
    );
    assert!(
        !cart.is_payment_cancelled(&payment_id),
        "not a card form closed"
    );
    h.finish().await;
}

#[tokio::test]
async fn a_payment_stripe_did_not_give_back_is_given_back_by_the_sweep() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let sweep = PendingPayments::new(h.db.clone(), h.store(&acme), h.payments.clone());
    let (cart_id, payment_id) = late_payment_stripe_cannot_give_back(&h, &acme).await;
    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();
    assert_eq!(timada_orders::money_owed(&h.db, &acme.id).await.unwrap(), 1);

    let now = now() as u64;
    // Whoever asked has a minute to finish it; Stripe still silent after.
    assert_eq!(sweep.sweep(now).await.unwrap(), 0);
    let later = now + ASKED_SECS + 1;
    assert_eq!(sweep.sweep(later).await.unwrap(), 0);
    h.stripe
        .unreachable
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        sweep.sweep(later).await.unwrap(),
        0,
        "asked again only later"
    );
    assert_eq!(sweep.sweep(later + RECHECK_SECS).await.unwrap(), 1);
    assert_eq!(
        *h.stripe.cancelled.lock().unwrap(),
        vec![payment_id.clone()]
    );
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.returned_payment(&payment_id).map(|r| r.code.as_str()),
        Some("out_of_stock")
    );
    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();
    assert_eq!(timada_orders::money_owed(&h.db, &acme.id).await.unwrap(), 0);
    h.finish().await;
}

#[tokio::test]
async fn money_taken_for_a_cancelled_order_is_refunded_through_it() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let sweep = PendingPayments::new(h.db.clone(), h.store(&acme), h.payments.clone());
    let account = h.account(&acme).await;
    // Goods a supplier buys: the money stays held. The merchant cancels.
    *h.goods.supplier.lock().unwrap() = Some("aliexpress".into());
    let order_id = h.place(&acme, checkout(Some(2500))).await;
    orders
        .request_cancellation("alice", &order_id, "Out of stock".into(), false)
        .await
        .unwrap();
    assert_eq!(
        payments::settle::settle_cancellation(&h.payments, &account, &orders, &order_id)
            .await
            .unwrap(),
        CancelOutcome::Cancelled
    );
    let payment_id = orders
        .load_order(&order_id)
        .await
        .unwrap()
        .unwrap()
        .payment
        .payment_id;

    // Taken by hand in Stripe's dashboard all the same; Stripe then does not
    // answer when asked to refund it.
    h.stripe
        .cancelled
        .lock()
        .unwrap()
        .retain(|id| *id != payment_id);
    h.stripe.captured.lock().unwrap().push(payment_id.clone());
    h.stripe
        .unreachable
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let body = succeeded(&payment_id);
    assert!(h.try_webhook(&acme, &body, signed(&body)).await.is_err());
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    let pending = order.pending_refund.clone().expect("asked for first");
    assert_eq!(pending.amount_cents, 2500);
    assert_eq!(order.status, OrderStatus::Cancelled);

    // The sweep makes it, under the key the order fixed.
    h.stripe
        .unreachable
        .store(false, std::sync::atomic::Ordering::SeqCst);
    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();
    assert_eq!(timada_orders::money_owed(&h.db, &acme.id).await.unwrap(), 1);
    assert_eq!(sweep.sweep(now() as u64 + ASKED_SECS + 1).await.unwrap(), 1);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.refunded_cents, 2500);
    assert_eq!(order.pending_refund, None);
    assert_eq!(
        *h.stripe.refunds.lock().unwrap(),
        vec![(payment_id.clone(), 2500)]
    );
    // Stripe telling again changes nothing.
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Refunded {
            refund_id: order.refund_ids[0].clone()
        }
    );
    assert_eq!(h.stripe.refunds.lock().unwrap().len(), 1);
    h.finish().await;
}

#[tokio::test]
async fn stopped_payments_settle_what_is_held_then_forget_the_keys() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let sweep = PendingPayments::new(h.db.clone(), h.store(&acme), h.payments.clone());
    let read = async || {
        timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
            .run_once(&h.platform.0)
            .await
            .unwrap();
        timada_tenancy::read_model_subscription::<cluster::Executor>(h.db.clone())
            .run_once(&h.platform.0)
            .await
            .unwrap();
    };
    // Goods a supplier buys: the order's money stays held.
    *h.goods.supplier.lock().unwrap() = Some("aliexpress".into());
    let order_id = h.place(&acme, checkout(Some(2500))).await;

    // The merchant stops taking payments: a new request to pay is refused,
    // the keys stay while the order's money is held.
    h.platform.stop_payments("alice", &acme.id).await.unwrap();
    let (cart_id, _) = h.cart(&acme).await;
    assert_eq!(
        h.ask(&acme, &cart_id, None).await,
        PaymentAnswer::Refused("payments_unavailable".into())
    );
    read().await;
    assert_eq!(timada_orders::money_owed(&h.db, &acme.id).await.unwrap(), 1);
    sweep.sweep(now() as u64).await.unwrap();
    let tenant = h.platform.load_tenant(&acme.id).await.unwrap().unwrap();
    assert!(tenant.payments.as_ref().unwrap().is_stopped(), "kept");

    // What is held is still settled with them: the merchant cancels.
    let account = h.payments.account_of(&tenant).unwrap().expect("kept");
    orders
        .request_cancellation("alice", &order_id, "Out of stock".into(), false)
        .await
        .unwrap();
    assert_eq!(
        payments::settle::settle_cancellation(&h.payments, &account, &orders, &order_id)
            .await
            .unwrap(),
        CancelOutcome::Cancelled
    );

    // Nothing held any more: the next sweep forgets the keys.
    read().await;
    assert_eq!(timada_orders::money_owed(&h.db, &acme.id).await.unwrap(), 0);
    sweep.sweep(now() as u64).await.unwrap();
    let tenant = h.platform.load_tenant(&acme.id).await.unwrap().unwrap();
    assert!(tenant.payments.is_none(), "forgotten");
    h.finish().await;
}

/// Two tabs, two payments, both confirmed: the cart is an order paid by the
/// first, and the second is taken in Stripe's own dashboard all the same.
async fn second_payment_taken(h: &Harness, tenant: &TenantState) -> (String, String) {
    let (cart_id, _) = h.cart(tenant).await;
    let PaymentAnswer::Requested(first) = h.ask(tenant, &cart_id, None).await else {
        panic!("a payment is opened");
    };
    let PaymentAnswer::Requested(second) = h.ask(tenant, &cart_id, None).await else {
        panic!("a payment is opened");
    };
    h.stripe.authorize(&first.payment_id);
    let body = authorized(&first.payment_id);
    assert!(matches!(
        h.webhook(tenant, &body, signed(&body)).await,
        Outcome::Placed { .. }
    ));
    h.stripe.authorize(&second.payment_id);
    h.stripe
        .captured
        .lock()
        .unwrap()
        .push(second.payment_id.clone());
    (cart_id, second.payment_id.clone())
}

#[tokio::test]
async fn a_payment_given_back_refunds_what_is_left_or_settles_as_refused() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);

    // 10.00 € of it went back by hand in Stripe: the rest goes back now.
    let (_, partly) = second_payment_taken(&h, &acme).await;
    h.stripe
        .refunds
        .lock()
        .unwrap()
        .push((partly.clone(), 1000));
    let body = succeeded(&partly);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Returned {
            code: "duplicate_payment".into()
        }
    );
    assert_eq!(
        *h.stripe.refunds.lock().unwrap(),
        vec![(partly.clone(), 1000), (partly.clone(), 1500)]
    );

    // Stripe will not refund the next one, for good: settled as refused,
    // shown to the merchant, never asked again.
    let (cart_id, disputed) = second_payment_taken(&h, &acme).await;
    h.stripe
        .refuses_refund
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let body = succeeded(&disputed);
    let refused = Outcome::NotReturned {
        reason: "This charge is disputed.".into(),
    };
    assert_eq!(h.webhook(&acme, &body, signed(&body)).await, refused);
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.return_refusal(&disputed),
        Some("This charge is disputed.")
    );
    assert_eq!(cart.pending_return(&disputed), None);
    assert_eq!(h.webhook(&acme, &body, signed(&body)).await, refused);
    assert_eq!(
        h.stripe
            .refund_attempts
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the refusal is settled: asked once"
    );
    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();
    assert!(
        timada_orders::returns_due(&h.db, i64::MAX, 10)
            .await
            .unwrap()
            .is_empty()
    );
    h.finish().await;
}

#[tokio::test]
async fn a_discount_code_is_charged_held_and_invoiced() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let store = h.store(&acme);
    let orders = timada_orders::Command::new(store.clone(), &acme.id);
    let discounts = timada_discounts::Command::new(store.clone(), &acme.id);
    let ten = discounts
        .create_discount(
            "alice",
            timada_discounts::CreateDiscount {
                code: "TEN".into(),
                value: timada_discounts::DiscountValue::Percent(1000),
                max_uses: Some(1),
                once_per_email: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let with_code = async |email: &str| {
        let (cart_id, _) = h.cart(&acme).await;
        orders
            .apply_discount(STOREFRONT_ACTOR, &cart_id, "ten")
            .await
            .unwrap();
        let checkout = Checkout {
            email: email.into(),
            ..checkout(Some(2250))
        };
        (cart_id.clone(), h.ask_with(&acme, &cart_id, checkout).await)
    };

    // Stripe fails: the use is given back.
    h.stripe
        .down
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (_, answer) = with_code("ada@example.org").await;
    assert_eq!(
        answer,
        PaymentAnswer::Refused("payments_unavailable".into())
    );
    let usage = discounts.load_usage(&ten.id).await.unwrap().unwrap();
    assert!(usage.reserved.is_empty());
    h.stripe
        .down
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // 10 % off two mugs at 12.50.
    let (cart_id, answer) = with_code("ada@example.org").await;
    let PaymentAnswer::Requested(payment) = answer else {
        panic!("a payment is opened: {answer:?}");
    };
    assert_eq!(payment.total_cents, 2250);
    assert_eq!(payment.discount.as_ref().unwrap().amount_cents, 250);
    {
        let opened = h.stripe.opened.lock().unwrap();
        let (_, new) = opened
            .values()
            .find(|(open, _)| open.payment_id == payment.payment_id)
            .unwrap();
        assert_eq!(new.amount_cents, 2250);
    }

    // The one use is held: someone else is refused, and so is Ada again.
    let (_, other) = with_code("bob@example.org").await;
    assert_eq!(other, PaymentAnswer::Refused("discount_exhausted".into()));
    let (_, again) = with_code("ada@example.org").await;
    assert_eq!(
        again,
        PaymentAnswer::Refused("discount_already_used".into())
    );

    h.stripe.authorize(&payment.payment_id);
    let body = authorized(&payment.payment_id);
    let Outcome::Placed { order_id } = h.webhook(&acme, &body, signed(&body)).await else {
        panic!("the order is placed");
    };
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 2250);
    assert_eq!(order.discount.as_ref().unwrap().code, "TEN");

    // The order spends the use for good.
    let usage = eventually(async || {
        discounts
            .load_usage(&ten.id)
            .await
            .unwrap()
            .filter(|usage| usage.consumed_by_cart(&cart_id).is_some())
    })
    .await;
    assert!(usage.reserved.is_empty());

    // The invoice keeps the lines at full price and says what came off.
    let invoicing = timada_invoicing::Command::new(store, &acme.id);
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    assert_eq!(invoice.lines[0].total_cents(), 2500);
    assert_eq!(invoice.discount.as_ref().unwrap().amount_cents, 250);
    assert_eq!(invoice.total_cents, 2250);
    assert_eq!(invoice.vat[0].base_cents, 1875);
    assert_eq!(invoice.vat[0].vat_cents, 375);
    assert!(
        timada_invoicing::pdf::render(&invoice)
            .unwrap()
            .starts_with(b"%PDF-")
    );
    h.finish().await;
}

#[tokio::test]
async fn a_checkout_offer_is_charged_and_invoiced() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let store = h.store(&acme);
    let (cart_id, mug) = h.cart(&acme).await;
    // The second mug at 5.00 instead of 12.50.
    timada_discounts::Command::new(store.clone(), &acme.id)
        .create_offer(
            "alice",
            timada_discounts::CreateOffer {
                product_id: mug,
                price_cents: 500,
                currency: "EUR".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let PaymentAnswer::Requested(payment) = h.ask(&acme, &cart_id, Some(1750)).await else {
        panic!("a payment is opened");
    };
    assert_eq!(payment.total_cents, 1750);
    let lines: Vec<(u64, u32)> = payment
        .lines
        .iter()
        .map(|line| (line.unit_amount_cents, line.quantity))
        .collect();
    assert_eq!(lines, vec![(500, 1), (1250, 1)]);
    h.stripe.authorize(&payment.payment_id);
    let body = authorized(&payment.payment_id);
    let Outcome::Placed { order_id } = h.webhook(&acme, &body, signed(&body)).await else {
        panic!("the order is placed");
    };

    // The invoice bills the unit at the offer's price: its total is the
    // charge.
    let invoicing = timada_invoicing::Command::new(store, &acme.id);
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    let billed: Vec<(u64, u32)> = invoice
        .lines
        .iter()
        .map(|line| (line.unit_amount_cents, line.quantity))
        .collect();
    assert_eq!(billed, vec![(500, 1), (1250, 1)]);
    assert_eq!(invoice.total_cents, 1750);
    assert_eq!(
        invoice
            .vat
            .iter()
            .map(|vat| vat.base_cents + vat.vat_cents)
            .sum::<u64>(),
        1750
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
        Some(seal::Sealer::new(
            "payments",
            "another-secret-of-thirty-two-chars!",
        )),
        h.stripe.clone(),
        h.platform.0.clone(),
    );
    assert!(other_node.account_of(&acme).is_err());
    let disabled = Payments::new(None, h.stripe.clone(), h.platform.0.clone());
    assert!(disabled.account_of(&acme).unwrap().is_none());
    h.finish().await;
}

#[tokio::test]
async fn the_tenant_store_learns_how_its_shoppers_pay() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let follow = async || {
        payments::setup_subscription(h.payments.clone(), h.store(&acme))
            .run_once(&h.platform.0)
            .await
            .unwrap();
    };
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    follow().await;
    assert_eq!(orders.payment_setup().await.unwrap(), None);

    // Connected: the public key reaches the tenant's store, nothing secret.
    let acme = h.connect_stripe(&acme).await;
    follow().await;
    assert_eq!(
        orders.payment_setup().await.unwrap(),
        Some(timada_orders::PaymentSetup {
            provider: "stripe".into(),
            publishable_key: "pk_test_123".into(),
        })
    );
    // Told again (another node, a replay), nothing new is written.
    let written = orders.load_payment_method().await.unwrap().unwrap();
    orders
        .set_payment_method(
            "timada-dashboard",
            Some(timada_orders::PaymentSetup {
                provider: "stripe".into(),
                publishable_key: "pk_test_123".into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        orders.load_payment_method().await.unwrap().unwrap(),
        written
    );

    // Stopped: the storefronts stop showing the payment form.
    h.platform.stop_payments("alice", &acme.id).await.unwrap();
    follow().await;
    assert_eq!(orders.payment_setup().await.unwrap(), None);
    h.finish().await;
}

/// The settings page tells the tenant's store at once and the payment-setup
/// subscription tells it again when it gets to the event: when both write at
/// the same time, the one that lost the race finds the same setup there and
/// has nothing left to do, so neither fails.
#[tokio::test]
async fn the_store_is_told_the_same_setup_twice_at_once() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let setup = timada_orders::PaymentSetup {
        provider: "stripe".into(),
        publishable_key: "pk_test_123".into(),
    };

    // Both load a store that knows nothing yet, so both write: the second
    // write lands on a stream the first one created.
    let mut follow = payments::setup_subscription(h.payments.clone(), h.store(&acme));
    let (page, subscription) = tokio::join!(
        payments::publish_setup(&h.payments, &acme, h.store(&acme)),
        follow.run_once(&h.platform.0),
    );
    page.unwrap();
    subscription.unwrap();
    assert_eq!(orders.payment_setup().await.unwrap(), Some(setup.clone()));
    let written = orders.load_payment_method().await.unwrap().unwrap();

    // The same when the stream exists already: the merchant stops payments
    // and two nodes tell the store at once.
    h.platform.stop_payments("alice", &acme.id).await.unwrap();
    let (first, second) = tokio::join!(
        orders.set_payment_method("timada-dashboard", None),
        orders.set_payment_method("timada-dashboard", None),
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(orders.payment_setup().await.unwrap(), None);
    let stopped = orders.load_payment_method().await.unwrap().unwrap();
    assert_eq!(
        stopped.aggregate_version().unwrap(),
        written.aggregate_version().unwrap() + 1
    );
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
    // stripe-mock keeps no state: what matters is that each call is sent
    // and its answer read.
    stripe
        .capture_payment("sk_test_123", &opened.payment_id)
        .await
        .unwrap();
    stripe
        .release_payment("sk_test_123", &opened.payment_id)
        .await
        .unwrap();
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

/// One checkout through Stripe itself, in test mode, with the operator's own
/// key (`STRIPE_TEST_SECRET_KEY`, `sk_test_…`): the payments subscription
/// opens a real PaymentIntent, held only, the test confirms it with Stripe's
/// test Visa card (`pm_card_visa`) as the Payment Element would, then hands
/// the webhook a signed `payment_intent.amount_capturable_updated`, and the
/// dashboard reads the payment back from Stripe before placing the order.
/// The shop holds the goods, so the money is taken at once. Cancelling
/// refunds it, and Stripe is asked whether the refund went through.
///
/// Ignored: it needs the network and a Stripe account. Run it with
/// `STRIPE_TEST_SECRET_KEY=sk_test_… cargo test -p timada-dashboard --test
/// payments -- --ignored`; without the variable it does nothing.
#[tokio::test]
#[ignore = "talks to Stripe's test mode: set STRIPE_TEST_SECRET_KEY"]
async fn a_checkout_goes_through_stripe_test_mode() {
    use stripe_core::PaymentIntentStatus;
    use stripe_core::payment_intent::ConfirmPaymentIntent;
    use stripe_core::refund::RetrieveRefund;

    // Empty counts as unset, as a CI secret that is not configured comes out.
    let Some(secret_key) = std::env::var("STRIPE_TEST_SECRET_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
    else {
        return;
    };
    // Never a live key: this test charges a card.
    assert!(
        secret_key.starts_with("sk_test_") || secret_key.starts_with("rk_test_"),
        "STRIPE_TEST_SECRET_KEY must be a test-mode key"
    );
    let gateway = Arc::new(payments::StripeGateway::new(None));
    gateway.check_key(&secret_key).await.unwrap();

    let h = harness_with(gateway, &secret_key).await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let (cart_id, _) = h.cart(&acme).await;
    let PaymentAnswer::Requested(payment) = h.ask(&acme, &cart_id, Some(2500)).await else {
        panic!("Stripe opens a payment");
    };
    assert!(payment.payment_id.starts_with("pi_"), "{payment:?}");
    assert_eq!(payment.total_cents, 2500);

    // Not confirmed yet: Stripe says so, and nothing is placed.
    let body = authorized(&payment.payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Ignored
    );

    // What the shopper's browser does through the Payment Element.
    let client = stripe::ClientBuilder::new(secret_key.as_str())
        .build()
        .unwrap();
    let confirmed = ConfirmPaymentIntent::new(payment.payment_id.as_str())
        .payment_method("pm_card_visa")
        .return_url("https://example.org/checkout/complete")
        .send(&client)
        .await
        .unwrap();
    assert_eq!(confirmed.status, PaymentIntentStatus::RequiresCapture);

    let Outcome::Placed { order_id } = h.webhook(&acme, &body, signed(&body)).await else {
        panic!("the order is placed");
    };
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let order = h.paid(&acme, &order_id).await;
    assert_eq!(order.status, timada_orders::OrderStatus::Paid);
    assert!(
        order.payment.capture_before.is_some(),
        "a card says until when"
    );
    assert_eq!(order.cart_id, cart_id);
    assert_eq!(order.total_cents, 2500);
    assert_eq!(order.payment.payment_id, payment.payment_id);
    assert_eq!(order.payment.amount_cents, 2500);
    assert_eq!(order.payment.currency, "EUR");

    // Cancelled as the order page does it: refunded through Stripe first.
    orders
        .request_cancellation("alice", &order_id, "Live test".into(), false)
        .await
        .unwrap();
    let account = h.account(&acme).await;
    assert_eq!(
        payments::settle::settle_cancellation(&h.payments, &account, &orders, &order_id)
            .await
            .unwrap(),
        CancelOutcome::Cancelled
    );
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, timada_orders::OrderStatus::Cancelled);
    assert_eq!(order.refunded_cents, 2500);
    let refund_id = order.refund_ids[0].clone();

    let refund = RetrieveRefund::new(refund_id.as_str())
        .send(&client)
        .await
        .unwrap();
    assert_eq!(refund.amount, 2500);
    assert_eq!(refund.status.as_deref(), Some("succeeded"), "{refund:?}");
    h.finish().await;
}

#[tokio::test]
async fn invoices_follow_where_the_goods_go() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let invoicing = timada_invoicing::Command::new(h.store(&acme), &acme.id);
    timada_orders::Command::new(h.store(&acme), &acme.id)
        .set_business_buyers("alice", true)
        .await
        .unwrap();
    let shipped_to = |country: &str, postal_code: &str, vat_number: Option<&str>| Checkout {
        shipping: ShippingAddress {
            country: country.into(),
            postal_code: postal_code.into(),
            ..checkout(None).shipping
        },
        vat_number: vat_number.map(str::to_owned),
        ..checkout(None)
    };

    // A German business VIES knows: no VAT, the reference kept.
    let business = h
        .buy(&acme, shipped_to("DE", "10115", Some("DE123456789")))
        .await;
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&business).await.unwrap()).await;
    assert_eq!(invoice.regime, timada_invoicing::tax::Regime::ReverseCharge);
    assert!(invoice.vat.is_empty());
    assert_eq!(invoice.buyer_vat_number.as_deref(), Some("DE123456789"));
    assert_eq!(invoice.vies_reference.as_deref(), Some("WAPI-DE123456789"));

    // A number VIES does not know: a consumer, French VAT under the threshold.
    let unknown = h
        .buy(&acme, shipped_to("DE", "10115", Some("DE999999999")))
        .await;
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&unknown).await.unwrap()).await;
    assert_eq!(
        invoice.regime,
        timada_invoicing::tax::Regime::DistanceOrigin
    );
    assert_eq!(invoice.vat[0].rate_bps, 2000);
    assert_eq!(invoice.buyer_vat_number, None);
    assert_eq!(invoice.eu_distance_sales_cents, 2083);

    // Out of the EU: an export, same price, no VAT.
    let export = h.buy(&acme, shipped_to("US", "10001", None)).await;
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&export).await.unwrap()).await;
    assert_eq!(invoice.regime, timada_invoicing::tax::Regime::Export);
    assert_eq!(invoice.total_cents, 2500);
    assert_eq!(invoice.total_excl_vat_cents, 2500);
    h.finish().await;
}

#[tokio::test]
async fn an_invoice_is_addressed_where_the_order_is_billed() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let invoicing = timada_invoicing::Command::new(h.store(&acme), &acme.id);

    // Billed in Paris, shipped to New York: addressed to Paris, taxed as
    // an export, since VAT follows the goods.
    let order_id = h
        .buy(
            &acme,
            Checkout {
                shipping: ShippingAddress {
                    line1: "350 Fifth Avenue".into(),
                    postal_code: "10118".into(),
                    city: "New York".into(),
                    region: None,
                    country: "US".into(),
                    ..checkout(None).shipping
                },
                billing: Some(ShippingAddress {
                    name: "Ada Corp".into(),
                    ..checkout(None).shipping
                }),
                ..checkout(None)
            },
        )
        .await;
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    assert_eq!(invoice.buyer.name, "Ada Corp");
    assert_eq!(invoice.buyer.city, "Paris");
    let delivery = invoice
        .delivery
        .expect("an invoice says where the goods went");
    assert_eq!(delivery.city, "New York");
    assert_eq!(delivery.name, "Ada");
    assert_eq!(invoice.regime, timada_invoicing::tax::Regime::Export);

    // Without a billing address, both are the shipping address.
    let order_id = h.buy(&acme, checkout(None)).await;
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    assert_eq!(invoice.buyer.name, "Ada");
    assert_eq!(invoice.delivery.as_ref(), Some(&invoice.buyer));
    h.finish().await;
}

#[tokio::test]
async fn shipping_is_paid_and_invoiced_at_the_rates_of_the_goods() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let method_id = orders
        .add_shipping_method(
            "alice",
            timada_orders::NewShippingMethod {
                name: "Tracked delivery".into(),
                price_cents: 490,
                currency: "EUR".into(),
                countries: vec!["FR".into()],
                free_above_cents: None,
            },
        )
        .await
        .unwrap();

    // Without a method picked, the request is refused.
    let (cart_id, _) = h.cart(&acme).await;
    let PaymentAnswer::Refused(code) = h.ask_with(&acme, &cart_id, checkout(None)).await else {
        panic!("a shop with methods needs one picked");
    };
    assert_eq!(code, "shipping_unavailable");

    let order_id = h
        .buy(
            &acme,
            Checkout {
                shipping_method: Some(method_id),
                ..checkout(Some(2500 + 490))
            },
        )
        .await;
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 2990);
    assert_eq!(order.payment.amount_cents, 2990);
    assert_eq!(order.shipping_charge.unwrap().name, "Tracked delivery");

    let invoicing = timada_invoicing::Command::new(h.store(&acme), &acme.id);
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    assert_eq!(invoice.total_cents, 2990);
    assert_eq!(invoice.shipping.unwrap().amount_cents, 490);
    // The mug is standard goods: shipping is at 20 % with it.
    assert_eq!(invoice.vat.len(), 1);
    assert_eq!(invoice.vat[0].rate_bps, 2000);
    assert_eq!(invoice.vat[0].base_cents + invoice.vat[0].vat_cents, 2990);
    h.finish().await;
}

/// Stripe would not take the money of an order (the card was cancelled
/// since): the order records why, has no invoice, and the merchant takes it
/// once more by hand.
#[tokio::test]
async fn a_refused_capture_is_recorded_and_taken_again_by_hand() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let invoicing = timada_invoicing::Command::new(h.store(&acme), &acme.id);
    h.stripe
        .refuses_capture
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let order_id = h.place(&acme, checkout(Some(2500))).await;
    let order = eventually(async || {
        orders
            .load_order(&order_id)
            .await
            .unwrap()
            .filter(|order| order.capture_failure.is_some())
    })
    .await;
    assert_eq!(order.status, OrderStatus::Authorized);
    assert_eq!(order.payment_state(), "capture_failed");
    assert_eq!(
        order.capture_failure.as_deref(),
        Some("Your card was declined.")
    );
    assert!(
        invoicing
            .load_invoice_of_order(&order_id)
            .await
            .unwrap()
            .is_none()
    );

    h.stripe
        .refuses_capture
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let claim = orders.request_capture("alice", &order_id).await.unwrap();
    assert!(matches!(claim, CaptureClaim::Claimed(_)), "{claim:?}");
    let account = h.account(&acme).await;
    assert_eq!(
        payments::settle::capture(&h.payments, &account, &orders, &order_id)
            .await
            .unwrap(),
        CaptureOutcome::Paid
    );
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(order.capture_failure, None);
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    assert_eq!(invoice.total_cents, 2500);
    h.finish().await;
}

/// Goods a supplier buys once the order is placed: the money stays held.
/// Cancelled before it is taken, the hold is given back with Stripe, the
/// goods return to the shelf, and there is neither invoice nor credit note.
#[tokio::test]
async fn a_held_order_is_cancelled_by_giving_its_hold_back() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let store = h.store(&acme);
    let orders = timada_orders::Command::new(store.clone(), &acme.id);
    let catalog = timada_catalog::Command::new(store.clone(), &acme.id);
    let invoicing = timada_invoicing::Command::new(store, &acme.id);
    *h.goods.supplier.lock().unwrap() = Some("aliexpress".into());
    let (cart_id, mug) = h.cart(&acme).await;
    catalog.set_stock("alice", &mug, None, 5).await.unwrap();

    let PaymentAnswer::Requested(payment) = h.ask(&acme, &cart_id, Some(2500)).await else {
        panic!("a payment is opened");
    };
    h.stripe.authorize(&payment.payment_id);
    let body = authorized(&payment.payment_id);
    let Outcome::Placed { order_id } = h.webhook(&acme, &body, signed(&body)).await else {
        panic!("the order is placed");
    };
    eventually(async || {
        catalog
            .load_stock(&mug, None)
            .await
            .unwrap()
            .filter(|stock| stock.on_hand == 3)
    })
    .await;
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Authorized);
    assert_eq!(order.supplier.as_deref(), Some("aliexpress"));
    assert!(order.awaits_supplier());
    assert!(
        h.stripe.captured.lock().unwrap().is_empty(),
        "nothing taken"
    );

    orders
        .request_cancellation("alice", &order_id, "Out of stock".into(), false)
        .await
        .unwrap();
    let account = h.account(&acme).await;
    assert_eq!(
        payments::settle::settle_cancellation(&h.payments, &account, &orders, &order_id)
            .await
            .unwrap(),
        CancelOutcome::Cancelled
    );
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancel_reason.as_deref(), Some("Out of stock"));
    assert!(order.released_at.is_some());
    assert_eq!(
        *h.stripe.cancelled.lock().unwrap(),
        vec![payment.payment_id.clone()]
    );
    assert!(h.stripe.refunds.lock().unwrap().is_empty());
    eventually(async || {
        catalog
            .load_stock(&mug, None)
            .await
            .unwrap()
            .filter(|stock| stock.on_hand == 5)
    })
    .await;
    assert!(
        invoicing
            .load_invoice_of_order(&order_id)
            .await
            .unwrap()
            .is_none()
    );
    // Stripe tells of the cancel it made: known already.
    let body = canceled(&payment.payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Ignored
    );
    h.finish().await;
}

/// A hold nobody took within its time is dropped by Stripe: the order is
/// cancelled when Stripe says so.
#[tokio::test]
async fn a_lapsed_hold_cancels_its_order() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    *h.goods.supplier.lock().unwrap() = Some("aliexpress".into());
    let order_id = h.place(&acme, checkout(Some(2500))).await;
    let order = orders.load_order(&order_id).await.unwrap().unwrap();

    h.stripe.lapse(&order.payment.payment_id);
    let body = canceled(&order.payment.payment_id);
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Cancelled {
            order_id: order_id.clone()
        }
    );
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancel_reason.as_deref(), Some(HOLD_EXPIRED_REASON));
    assert!(order.released_at.is_some());
    // Told again: nothing more.
    assert_eq!(
        h.webhook(&acme, &body, signed(&body)).await,
        Outcome::Ignored
    );
    h.finish().await;
}

/// Stripe did not answer when it was asked: the capture or the cancellation
/// stands on the order, and the sweep settles it once Stripe answers.
#[tokio::test]
async fn what_stripe_did_not_answer_is_settled_by_the_sweep() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let orders = timada_orders::Command::new(h.store(&acme), &acme.id);
    let sweep = PendingPayments::new(h.db.clone(), h.store(&acme), h.payments.clone());
    let account = h.account(&acme).await;
    let unreachable = |down: bool| {
        h.stripe
            .unreachable
            .store(down, std::sync::atomic::Ordering::SeqCst)
    };

    // Goods the shop holds: the capture is asked and gets no answer.
    unreachable(true);
    let bought = h.place(&acme, checkout(Some(2500))).await;
    eventually(async || {
        orders
            .load_order(&bought)
            .await
            .unwrap()
            .filter(|order| order.pending_capture.is_some())
    })
    .await;
    // Goods a supplier buys: the merchant cancels, Stripe does not answer.
    *h.goods.supplier.lock().unwrap() = Some("aliexpress".into());
    let held = h.place(&acme, checkout(Some(2500))).await;
    orders
        .request_cancellation("alice", &held, "Out of stock".into(), false)
        .await
        .unwrap();
    assert!(matches!(
        payments::settle::settle_cancellation(&h.payments, &account, &orders, &held)
            .await
            .unwrap(),
        CancelOutcome::Unanswered(_)
    ));

    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(&h.platform.0)
        .await
        .unwrap();
    let now = now() as u64;
    // Whoever asked has a minute to settle it.
    assert_eq!(sweep.sweep(now).await.unwrap(), 0);
    // Stripe still silent: left for later.
    let later = now + ASKED_SECS + 1;
    assert_eq!(sweep.sweep(later).await.unwrap(), 0);
    unreachable(false);
    assert_eq!(
        sweep.sweep(later).await.unwrap(),
        0,
        "asked again only later"
    );
    assert_eq!(sweep.sweep(later + RECHECK_SECS).await.unwrap(), 2);
    assert_eq!(
        orders.load_order(&bought).await.unwrap().unwrap().status,
        OrderStatus::Paid
    );
    let held = orders.load_order(&held).await.unwrap().unwrap();
    assert_eq!(held.status, OrderStatus::Cancelled);
    assert!(held.released_at.is_some());
    h.finish().await;
}

/// Goods bought on AliExpress once the order is placed: the money stays held
/// until AliExpress took the order, then it is taken and invoiced.
#[tokio::test]
async fn an_order_placed_on_aliexpress_is_paid_then() {
    let h = harness().await;
    let acme = h.create("acme").await;
    let acme = h.connect_stripe(&acme).await;
    let store = h.store(&acme);
    let orders = timada_orders::Command::new(store.clone(), &acme.id);
    let invoicing = timada_invoicing::Command::new(store.clone(), &acme.id);
    *h.goods.supplier.lock().unwrap() = Some("aliexpress".into());
    let order_id = h.place(&acme, checkout(Some(2500))).await;
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Authorized);
    assert!(order.awaits_supplier());

    // The forwarder writes its fence, AliExpress takes the order.
    orders
        .begin_supplier_order("timada-aliexpress", &order_id, "aliexpress", now() as u64)
        .await
        .unwrap();
    let forwards = timada_aliexpress::Command::new(store, &acme.id);
    let forward_id = forwards
        .request_forward("timada-aliexpress", &order_id, false)
        .await
        .unwrap();
    assert!(
        forwards
            .start_forward("timada-aliexpress", &forward_id)
            .await
            .unwrap()
    );
    assert!(h.stripe.captured.lock().unwrap().is_empty(), "still held");
    forwards
        .record_placed(
            "timada-aliexpress",
            &forward_id,
            timada_aliexpress::Placement {
                ae_order_ids: vec!["8100000000000001".into()],
                items: Vec::new(),
                pay_requested: true,
                complete: true,
            },
        )
        .await
        .unwrap();

    let order = h.paid(&acme, &order_id).await;
    assert_eq!(order.paid_cents, 2500);
    assert_eq!(
        order.pending_capture, None,
        "taken by the capture subscription"
    );
    let invoice =
        eventually(async || invoicing.load_invoice_of_order(&order_id).await.unwrap()).await;
    assert_eq!(invoice.total_cents, 2500);
    h.finish().await;
}
