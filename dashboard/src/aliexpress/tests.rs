//! Reads of linked items against a stand-in for AliExpress, on a store of
//! their own: what a read writes, which node reads, what the sweep asks for.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use evento::Evento;
use timada_aliexpress::aggregator::SkuLink;
use timada_aliexpress::{AeProduct, map_product, track_imported};
use timada_catalog_import::{ImportOptions, NoImages, import_one};
use timada_core::Share;
use timada_orders::{PROVIDER_STRIPE, Payment};
use timada_tenancy::{
    AuthorizeAliexpress, ConfigureAliexpress, ConfigurePayments, CreateTenant, LegalIdentity,
    Pricing, PricingRule, Rounding, TenantState,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

use super::payment_reads::AliexpressGoods;
use super::subscription::reads_subscription;
use super::sync::AliexpressSync;
use super::{ALIEXPRESS_ACTOR, Aliexpress, SyncMode, SyncResult};
use crate::config::AliexpressConfig;
use crate::db::Db;
use crate::payments::fake::FakeGateway;
use crate::payments::{Payments, SuppliedGoods};
use crate::seal::Sealer;

const AE_PRODUCT: &str = "1005006123456789";
const ALICE: &str = "alice";
/// How often the sweep's timer runs, and how soon an order with every parcel
/// known is read again.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// AliExpress, as far as reading one product goes: two combinations, of which
/// a test changes the first.
struct StandIn {
    /// What the first combination costs, as AliExpress writes it.
    cost: Mutex<String>,
    stock: AtomicU32,
    offline: AtomicBool,
    /// Answers 503 to everything.
    down: AtomicBool,
    /// Keeps every caller waiting.
    hung: AtomicBool,
    /// Holds each answer until told to go.
    held: AtomicBool,
    /// Told when an answer is being held.
    holding: Notify,
    /// Lets a held answer go.
    answer: Notify,
    reads: AtomicUsize,
    /// How many times delivery was asked about (`aliexpress.ds.freight.query`).
    freights: AtomicUsize,
    /// What delivering one costs, in cents; free by default.
    delivery_cents: AtomicU64,
    /// The orders placed, each as [`PLACED_AE_ORDER`].
    placed: AtomicUsize,
    /// How the next orders placed are answered, in turn; taken once none is
    /// left.
    place_answers: Mutex<std::collections::VecDeque<PlaceAnswer>>,
}

/// How the stand-in answers an order placed.
#[derive(Debug, Clone, Copy)]
enum PlaceAnswer {
    /// Taken, as [`PLACED_AE_ORDER`].
    Taken,
    /// Taken, the answer saying no number: AliExpress may have it.
    Lost,
    /// Surely refused.
    Refused,
}

/// The number AliExpress gives every order placed with the stand-in.
const PLACED_AE_ORDER: &str = "8100000000000007";

impl StandIn {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cost: Mutex::new("4.31".to_owned()),
            stock: AtomicU32::new(120),
            offline: AtomicBool::new(false),
            down: AtomicBool::new(false),
            hung: AtomicBool::new(false),
            held: AtomicBool::new(false),
            holding: Notify::new(),
            answer: Notify::new(),
            reads: AtomicUsize::new(0),
            freights: AtomicUsize::new(0),
            delivery_cents: AtomicU64::new(0),
            placed: AtomicUsize::new(0),
            place_answers: Mutex::default(),
        })
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    /// Listens on a port of its own; its address.
    async fn start(self: Arc<Self>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let stand_in = self.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut chunk = [0; 4096];
                    while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => request.extend_from_slice(&chunk[..read]),
                        }
                    }
                    // A delivery lookup goes with each read: answered at
                    // once, it is not one.
                    let line = request
                        .split(|byte| *byte == b'\n')
                        .next()
                        .unwrap_or_default();
                    if String::from_utf8_lossy(line).contains("method=aliexpress.ds.freight.query")
                    {
                        stand_in.freights.fetch_add(1, Ordering::SeqCst);
                        let body = stand_in.freight().to_string();
                        let answer = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        socket.write_all(answer.as_bytes()).await.unwrap();
                        return;
                    }
                    // Placing an order is no read either.
                    if String::from_utf8_lossy(line).contains("method=aliexpress.ds.order.create") {
                        stand_in.placed.fetch_add(1, Ordering::SeqCst);
                        let answer = stand_in
                            .place_answers
                            .lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or(PlaceAnswer::Taken);
                        let result = match answer {
                            PlaceAnswer::Taken => serde_json::json!({
                                "is_success": true, "order_list": { "number": [PLACED_AE_ORDER] },
                            }),
                            PlaceAnswer::Lost => {
                                serde_json::json!({ "is_success": true, "order_list": [] })
                            }
                            PlaceAnswer::Refused => serde_json::json!({
                                "is_success": false,
                                "error_code": "B_DROPSHIPPER_DELIVERY_ADDRESS_VALIDATE_FAIL",
                                "error_msg": "address",
                            }),
                        };
                        let body = serde_json::json!({
                            "aliexpress_ds_order_create_response": { "result": result },
                        })
                        .to_string();
                        let answer = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        socket.write_all(answer.as_bytes()).await.unwrap();
                        return;
                    }
                    stand_in.reads.fetch_add(1, Ordering::SeqCst);
                    if stand_in.hung.load(Ordering::SeqCst) {
                        return std::future::pending::<()>().await;
                    }
                    if stand_in.held.load(Ordering::SeqCst) {
                        stand_in.holding.notify_one();
                        stand_in.answer.notified().await;
                    }
                    let answer = if stand_in.down.load(Ordering::SeqCst) {
                        "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
                    } else {
                        let body = stand_in.product().to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    };
                    socket.write_all(answer.as_bytes()).await.unwrap();
                });
            }
        });
        url
    }

    /// The answer to `aliexpress.ds.freight.query`: AliExpress ships it.
    fn freight(&self) -> serde_json::Value {
        let cents = self.delivery_cents.load(Ordering::SeqCst);
        serde_json::json!({ "aliexpress_ds_freight_query_response": { "result": {
            "success": true,
            "delivery_options": { "delivery_option_d_t_o": [{
                "code": "CAINIAO_STANDARD",
                "free_shipping": cents == 0,
                "shipping_fee_cent": cents.to_string(),
                "shipping_fee_currency": "EUR",
                "tracking": true,
                "min_delivery_days": 7,
                "max_delivery_days": 12,
            }] },
        } } })
    }

    /// The answer to `aliexpress.ds.product.get`.
    fn product(&self) -> serde_json::Value {
        let status = if self.offline.load(Ordering::SeqCst) {
            "offline"
        } else {
            "onSelling"
        };
        let sku = |id: &str, colour: &str, cost: &str, stock: u32| {
            serde_json::json!({
                "sku_id": id,
                "sku_attr": format!("14:1#{colour}"),
                "offer_sale_price": cost,
                "currency_code": "EUR",
                "sku_available_stock": stock,
                "sku_stock": stock > 0,
                "ae_sku_property_dtos": { "ae_sku_property_d_t_o": [
                    { "sku_property_name": "Colour", "sku_property_value": colour },
                ] },
            })
        };
        serde_json::json!({ "aliexpress_ds_product_get_response": {
            "rsp_code": "200",
            "result": {
                "ae_item_base_info_dto": {
                    "product_id": AE_PRODUCT.parse::<u64>().unwrap(),
                    "subject": "Cat Tunnel",
                    "detail": "<p>Crinkly.</p>",
                    "currency_code": "CNY",
                    "product_status_type": status,
                },
                "ae_item_sku_info_dtos": { "ae_item_sku_info_d_t_o": [
                    sku("12000037001", "Rose", &self.cost.lock().unwrap(), self.stock.load(Ordering::SeqCst)),
                    sku("12000037002", "Blue", "5.50", 9),
                ] },
            },
        } })
    }
}

/// A shop selling as a company whose account is connected, one item imported.
struct Shop {
    store: Evento,
    db: Db,
    aliexpress: Arc<Aliexpress>,
    stand_in: Arc<StandIn>,
    /// The payment provider, holding the payments of the orders to place.
    provider: Arc<FakeGateway>,
    payments: Arc<Payments>,
    /// Which of the nodes up this one is.
    share: Arc<Mutex<Share>>,
    tenant: TenantState,
    link_id: String,
    product_id: String,
}

impl Shop {
    async fn new() -> Self {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        let db = Db::from(turso.clone());
        let store = Evento::new(turso);
        let stand_in = StandIn::new();
        let share = Arc::new(Mutex::new(Share::ALONE));
        let current = share.clone();
        let quota = Arc::new(crate::media::Quota::new(db.clone(), None));
        let aliexpress = Aliexpress::new(
            Some(Sealer::new("aliexpress-test", "secret")),
            &AliexpressConfig {
                app_key: Some("key".to_owned()),
                app_secret: Some("secret".to_owned()),
                api_url: Some(stand_in.clone().start().await),
                authorize_url: None,
                sync_check_secs: 3600,
                sync_every_secs: 3600,
                order_read_every_secs: 86_400,
                parcel_read_every_secs: 1,
                calls_per_day: 86_400_000,
                read_fresh_secs: 60,
            },
            false,
            store.clone(),
            quota,
            Arc::new(move |_| *current.lock().unwrap()),
        )
        .unwrap();

        let tenancy = timada_tenancy::Command(store.clone());
        let tenant = tenancy
            .create_tenant(CreateTenant {
                name: "Acme".into(),
                owner_user_id: ALICE.into(),
            })
            .await
            .unwrap();
        let company = tenancy
            .create_company(
                ALICE,
                LegalIdentity {
                    legal_name: "Acme SAS".into(),
                    legal_form: "SAS".into(),
                    siret: "12002701600092".into(),
                    line1: "1 rue de la Paix".into(),
                    postal_code: "75002".into(),
                    city: "Paris".into(),
                    country: "FR".into(),
                    contact_email: "shop@example.org".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        tenancy
            .link_tenant(ALICE, &company.id, &tenant.id, None)
            .await
            .unwrap();
        let provider = FakeGateway::new();
        let payments = Arc::new(Payments::new(
            Some(Sealer::new("payments", "secret")),
            provider.clone(),
            store.clone(),
        ));
        tenancy
            .configure_payments(
                ALICE,
                &tenant.id,
                ConfigurePayments {
                    provider: PROVIDER_STRIPE.into(),
                    publishable_key: "pk_test_123".into(),
                    secret_key_sealed: payments.seal(&tenant.id, "sk_test_good").unwrap(),
                    webhook_secret_sealed: payments.seal(&tenant.id, "whsec_test").unwrap(),
                },
            )
            .await
            .unwrap();
        tenancy
            .authorize_aliexpress(
                ALICE,
                &company.id,
                AuthorizeAliexpress {
                    access_token_sealed: aliexpress.seal(&company.id, "token").unwrap(),
                    refresh_token_sealed: aliexpress.seal(&company.id, "refresh").unwrap(),
                    expires_at: crate::auth::now() + 30 * 86_400,
                    refresh_expires_at: 0,
                    account: "shop@example.org".into(),
                },
            )
            .await
            .unwrap();
        let pricing = Pricing::flat(PricingRule {
            multiplier_bps: 20_000,
            plus_cents: 0,
            rounding: Rounding::Cent,
        });
        tenancy
            .configure_aliexpress(
                ALICE,
                &tenant.id,
                ConfigureAliexpress {
                    ship_to_country: "FR".into(),
                    currency: "EUR".into(),
                    language: "fr".into(),
                    pricing: Some(pricing.clone()),
                    price_tolerance_bps: 0,
                },
            )
            .await
            .unwrap();
        let tenant = tenancy.load_tenant(&tenant.id).await.unwrap().unwrap();

        // The import, as the subscription makes it.
        let catalog = timada_catalog::Command::new(store.clone(), &tenant.id);
        let links = timada_aliexpress::Command::new(store.clone(), &tenant.id);
        let link_id = links.request_import(ALICE, AE_PRODUCT).await.unwrap();
        let product = AeProduct::from_response(AE_PRODUCT, &stand_in.product()).unwrap();
        let mapped = map_product(&product, "EUR", Some(&pricing)).unwrap();
        let imported = import_one(
            &catalog,
            ALICE,
            &mapped.seed,
            Path::new(""),
            Arc::new(NoImages),
            &ImportOptions {
                publish: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let skus = track_imported(
            &catalog,
            ALICE,
            &imported.product_id,
            &imported.variant_ids,
            &mapped.skus,
        )
        .await
        .unwrap();
        links
            .record_import(ALICE, &link_id, &imported.product_id, skus)
            .await
            .unwrap();

        let shop = Self {
            store,
            db,
            aliexpress,
            stand_in,
            provider,
            payments,
            share,
            tenant,
            link_id,
            product_id: imported.product_id,
        };
        shop.follow_read_models().await;
        shop
    }

    fn links(&self) -> timada_aliexpress::Command<Evento> {
        timada_aliexpress::Command::new(self.store.clone(), &self.tenant.id)
    }

    /// How many events the link's stream holds.
    async fn version(&self) -> u16 {
        self.links()
            .load_link_versioned(&self.link_id)
            .await
            .unwrap()
            .unwrap()
            .1
    }

    /// Reads the item now.
    async fn sync(&self, mode: SyncMode<'_>) -> SyncResult {
        let company = self
            .aliexpress
            .company_of(&self.tenant)
            .await
            .unwrap()
            .unwrap();
        self.aliexpress
            .sync_link(
                &self.db,
                &self.store,
                &self.tenant,
                &company,
                &self.link_id,
                mode,
            )
            .await
            .unwrap()
    }

    /// The price and stock of the first combination, in the catalog.
    async fn rose(&self) -> (u64, u32) {
        let catalog = timada_catalog::Command::new(self.store.clone(), &self.tenant.id);
        let product = catalog
            .load_product(&self.product_id)
            .await
            .unwrap()
            .unwrap();
        let rose = &product.variants[0];
        let stock = catalog
            .load_stock(&self.product_id, Some(&rose.id))
            .await
            .unwrap()
            .unwrap();
        (rose.price.amount_cents, stock.on_hand)
    }

    /// The row of the item in the read model.
    async fn row(&self) -> timada_aliexpress::LinkRow {
        self.follow_read_models().await;
        timada_aliexpress::link_of_product(&self.db, &self.tenant.id, &self.product_id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn follow_read_models(&self) {
        timada_aliexpress::read_model_subscription::<Evento>(self.db.clone())
            .run_once(&self.store)
            .await
            .unwrap();
    }

    /// The node's subscription catches up with the requests.
    async fn follow_requests(&self) {
        reads_subscription::<Evento>(self.aliexpress.clone(), self.db.clone(), self.store.clone())
            .run_once(&self.store)
            .await
            .unwrap();
    }

    /// What a payment asks for its cart holding the item.
    fn goods(&self) -> AliexpressGoods {
        AliexpressGoods::new(self.aliexpress.clone(), self.db.clone(), self.store.clone())
    }

    /// Sets when the item was last read, as this node knows it: the read
    /// its stream recorded (the import) and the node's own record of it.
    async fn read_at(&self, at: u64) {
        self.db
            .execute(
                "UPDATE aliexpress_products SET last_synced_at = ?1, last_tried_at = ?1",
                (at as i64,),
            )
            .await
            .unwrap();
        self.db
            .execute(
                "INSERT INTO aliexpress_read_times (tenant_id, id, tried_at, synced_at)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET
                    tried_at = excluded.tried_at, synced_at = excluded.synced_at",
                (self.tenant.id.as_str(), self.link_id.as_str(), at as i64),
            )
            .await
            .unwrap();
    }

    /// A shopper puts the first combination in a cart. The item is on sale.
    async fn add_to_cart(&self) {
        let catalog = timada_catalog::Command::new(self.store.clone(), &self.tenant.id);
        let product = catalog
            .load_product(&self.product_id)
            .await
            .unwrap()
            .unwrap();
        if !product.is_published() {
            catalog
                .publish_product(ALICE, &self.product_id)
                .await
                .unwrap();
        }
        let orders = timada_orders::Command::new(self.store.clone(), &self.tenant.id);
        orders
            .start_cart(
                timada_orders::STOREFRONT_ACTOR,
                timada_orders::CartItem {
                    product_id: self.product_id.clone(),
                    variant_id: Some(product.variants[0].id.clone()),
                    quantity: 1,
                },
            )
            .await
            .unwrap();
    }

    /// A node of two that does not hold the item.
    fn elsewhere(&self) -> Share {
        share_without(&self.link_id)
    }
}

/// A node of two that does not hold the item or order `id`.
fn share_without(id: &str) -> Share {
    let holder = (0..2)
        .find(|&index| Share { index, of: 2 }.owns(id))
        .unwrap();
    Share {
        index: 1 - holder,
        of: 2,
    }
}

#[tokio::test]
async fn a_read_that_finds_nothing_new_writes_nothing_and_notes_when_it_was() {
    let shop = Shop::new().await;
    let version = shop.version().await;
    shop.read_at(100).await;
    let before = crate::auth::now() as i64;

    let result = shop.sync(SyncMode::Routine).await;

    assert_eq!(
        result,
        SyncResult::Synced {
            offline: false,
            repriced: 0
        }
    );
    assert_eq!(shop.stand_in.reads(), 1);
    assert_eq!(shop.version().await, version, "no event on the link");
    let row = shop.row().await;
    assert!(
        row.last_synced_at >= Some(before),
        "{:?}",
        row.last_synced_at
    );
    assert!(row.last_tried_at >= Some(before), "{:?}", row.last_tried_at);
}

#[tokio::test]
async fn a_read_that_only_finds_the_stock_moved_counts_it_in_the_catalog_alone() {
    let shop = Shop::new().await;
    let version = shop.version().await;
    assert_eq!(shop.rose().await, (862, 120));

    shop.stand_in.stock.store(7, Ordering::SeqCst);
    shop.sync(SyncMode::Routine).await;

    assert_eq!(shop.rose().await, (862, 7));
    assert_eq!(shop.version().await, version, "no event on the link");
}

#[tokio::test]
async fn a_read_that_finds_a_new_cost_records_it_and_follows_the_rule() {
    let shop = Shop::new().await;
    let version = shop.version().await;

    *shop.stand_in.cost.lock().unwrap() = "5.00".to_owned();
    let result = shop.sync(SyncMode::Routine).await;

    assert_eq!(
        result,
        SyncResult::Synced {
            offline: false,
            repriced: 1
        }
    );
    assert_eq!(shop.rose().await.0, 1000);
    assert_eq!(shop.version().await, version + 1);
    let link = shop
        .links()
        .load_link_by_id(&shop.link_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(link.skus[0].cost_cents, 500);
}

#[tokio::test]
async fn a_read_prices_the_delivery_aliexpress_charges_in() {
    let shop = Shop::new().await;

    shop.stand_in.delivery_cents.store(500, Ordering::SeqCst);
    let result = shop.sync(SyncMode::Asked { by: ALICE }).await;

    assert!(matches!(result, SyncResult::Synced { .. }), "{result:?}");
    assert_eq!(shop.rose().await.0, 1862, "(4.31 + 5.00) × 2");
    let link = shop
        .links()
        .load_link_by_id(&shop.link_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(link.skus[0].cost_cents, 931);
    assert_eq!(shop.stand_in.reads(), 1, "the delivery lookup is no read");
}

#[tokio::test]
async fn a_read_brings_in_the_days_the_delivery_takes() {
    let shop = Shop::new().await;

    let result = shop.sync(SyncMode::Asked { by: ALICE }).await;

    assert!(matches!(result, SyncResult::Synced { .. }), "{result:?}");
    let product = timada_catalog::Command::new(shop.store.clone(), &shop.tenant.id)
        .load_product(&shop.product_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        product.delivery_days,
        Some(timada_catalog::DeliveryDays::new(7, 12).unwrap())
    );
}

#[tokio::test]
async fn a_read_a_member_waits_for_records_its_outcome_even_when_nothing_changed() {
    let shop = Shop::new().await;
    let version = shop.version().await;

    shop.sync(SyncMode::Asked { by: ALICE }).await;

    assert_eq!(shop.version().await, version + 1);
    let link = shop
        .links()
        .load_link_by_id(&shop.link_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(link.answered_version, version + 1);
}

#[tokio::test]
async fn a_request_during_a_read_leaves_the_read_its_record() {
    let shop = Arc::new(Shop::new().await);
    *shop.stand_in.cost.lock().unwrap() = "5.00".to_owned();
    shop.stand_in.held.store(true, Ordering::SeqCst);
    let read = tokio::spawn({
        let shop = shop.clone();
        async move { shop.sync(SyncMode::Asked { by: ALICE }).await }
    });
    shop.stand_in.holding.notified().await;

    // The sweep asks while the member's read is out: it changes nothing the
    // read knew of.
    let asked = shop
        .links()
        .request_sync(ALIEXPRESS_ACTOR, &shop.link_id)
        .await
        .unwrap();
    shop.stand_in.answer.notify_one();

    assert_eq!(
        read.await.unwrap(),
        SyncResult::Synced {
            offline: false,
            repriced: 1
        }
    );
    let link = shop
        .links()
        .load_link_by_id(&shop.link_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        link.answered_version,
        asked + 1,
        "the member has their answer"
    );
    assert_eq!(link.skus[0].cost_cents, 500);
}

#[tokio::test]
async fn a_combination_added_during_a_read_wins_over_the_reads_record() {
    let shop = Arc::new(Shop::new().await);
    *shop.stand_in.cost.lock().unwrap() = "5.00".to_owned();
    shop.stand_in.held.store(true, Ordering::SeqCst);
    let read = tokio::spawn({
        let shop = shop.clone();
        async move { shop.sync(SyncMode::Routine).await }
    });
    shop.stand_in.holding.notified().await;

    // The read does not know this one: recorded, it would forget it.
    let version = shop.version().await;
    let added = SkuLink {
        sku_id: "12000037009".to_owned(),
        variant_id: Some("green".to_owned()),
        cost_cents: 300,
    };
    shop.links()
        .record_combinations_added(ALICE, &shop.link_id, version, vec![added.clone()])
        .await
        .unwrap();
    shop.stand_in.answer.notify_one();

    assert_eq!(read.await.unwrap(), SyncResult::Skipped);
    let link = shop
        .links()
        .load_link_by_id(&shop.link_id)
        .await
        .unwrap()
        .unwrap();
    assert!(link.skus.contains(&added));
    assert_eq!(
        shop.rose().await.0,
        1000,
        "the catalog is in step all the same"
    );
}

#[tokio::test]
async fn the_same_failure_is_recorded_once_and_the_next_read_clears_it() {
    let shop = Shop::new().await;
    let version = shop.version().await;
    shop.stand_in.down.store(true, Ordering::SeqCst);

    assert!(matches!(
        shop.sync(SyncMode::Routine).await,
        SyncResult::Failed(_)
    ));
    assert_eq!(shop.version().await, version + 1);
    assert!(matches!(
        shop.sync(SyncMode::Routine).await,
        SyncResult::Failed(_)
    ));
    assert_eq!(shop.version().await, version + 1, "the same reason again");
    let row = shop.row().await;
    assert!(row.last_error.is_some());

    // A member asked: they read what became of their read.
    shop.sync(SyncMode::Asked { by: ALICE }).await;
    assert_eq!(shop.version().await, version + 2);

    // Back: nothing changed, but the problem is no more.
    shop.stand_in.down.store(false, Ordering::SeqCst);
    shop.sync(SyncMode::Routine).await;
    assert_eq!(shop.version().await, version + 3);
    assert_eq!(shop.row().await.last_error, None);
}

#[tokio::test]
async fn a_request_is_read_by_the_node_the_item_belongs_to() {
    let shop = Shop::new().await;
    shop.links()
        .request_sync(ALIEXPRESS_ACTOR, &shop.link_id)
        .await
        .unwrap();

    // Not this node's: left to the other.
    *shop.share.lock().unwrap() = shop.elsewhere();
    shop.follow_requests().await;
    assert_eq!(shop.stand_in.reads(), 0);

    // A later one, this node's.
    *shop.share.lock().unwrap() = Share::ALONE;
    shop.links()
        .request_sync(ALIEXPRESS_ACTOR, &shop.link_id)
        .await
        .unwrap();
    shop.follow_requests().await;
    assert_eq!(shop.stand_in.reads(), 1);
}

#[tokio::test]
async fn who_asked_decides_what_the_read_records() {
    let shop = Shop::new().await;

    // The dashboard on its own: nothing changed, nothing more than the request.
    let asked = shop
        .links()
        .request_sync(ALIEXPRESS_ACTOR, &shop.link_id)
        .await
        .unwrap();
    shop.follow_requests().await;
    assert_eq!(shop.version().await, asked);

    // A member: what comes after the request says how it went.
    let asked = shop
        .links()
        .request_sync(ALICE, &shop.link_id)
        .await
        .unwrap();
    shop.follow_requests().await;
    assert_eq!(shop.version().await, asked + 1);
}

#[tokio::test]
async fn the_sweep_asks_once_a_period_for_what_is_due() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let now = crate::auth::now();

    assert_eq!(sweep.ask_due(now).await.unwrap(), 0, "just imported");

    let later = now + shop.aliexpress.sync_every_secs() + 1;
    let version = shop.version().await;
    assert_eq!(sweep.ask_due(later).await.unwrap(), 1);
    assert_eq!(shop.version().await, version + 1);
    assert_eq!(sweep.ask_due(later).await.unwrap(), 0, "asked already");
    assert_eq!(shop.version().await, version + 1);

    // A node holding another share leaves it alone.
    *shop.share.lock().unwrap() = shop.elsewhere();
    let after = later + shop.aliexpress.sync_every_secs() + 1;
    assert_eq!(sweep.ask_due(after).await.unwrap(), 0);
}

#[tokio::test]
async fn a_shop_that_cannot_read_is_passed_over_without_a_request() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let tenancy = timada_tenancy::Command(shop.store.clone());
    tenancy
        .remove_aliexpress(ALICE, &shop.tenant.id)
        .await
        .unwrap();
    let later = crate::auth::now() + shop.aliexpress.sync_every_secs() + 1;
    let version = shop.version().await;

    assert_eq!(sweep.ask_due(later).await.unwrap(), 1);

    assert_eq!(shop.version().await, version, "no request");
    assert_eq!(shop.row().await.last_tried_at, Some(later as i64));
    assert_eq!(shop.stand_in.reads(), 0);
    assert_eq!(sweep.ask_due(later).await.unwrap(), 0, "passed over");
}

#[tokio::test]
async fn the_read_model_hears_what_makes_a_product_matter() {
    let shop = Shop::new().await;
    let matters = async || -> (bool, Option<i64>) {
        shop.follow_read_models().await;
        shop.db
            .fetch_one("SELECT published, wanted_at FROM aliexpress_products", ())
            .await
            .unwrap()
    };
    assert_eq!(matters().await, (false, None), "a draft nobody wants");

    // On sale, and in a cart.
    shop.add_to_cart().await;
    let (published, wanted_at) = matters().await;
    assert!(published);
    let in_cart = wanted_at.unwrap();

    // Bought: an order names its lines.
    evento::append("an-order")
        .tenant(&shop.tenant.id)
        .original_version(0)
        .event(&timada_orders::aggregator::Placed {
            reference: "K7Q4-MZP2".to_owned(),
            cart_id: "a-cart".to_owned(),
            email: "ada@example.com".to_owned(),
            shipping: Default::default(),
            note: None,
            lines: vec![timada_orders::OrderLine {
                product_id: shop.product_id.clone(),
                quantity: 1,
                ..Default::default()
            }],
            currency: "EUR".to_owned(),
            total_cents: 862,
            payment: Default::default(),
            supplier: Some(timada_aliexpress::ALIEXPRESS_SUPPLIER.to_owned()),
            placed_by: "storefront".to_owned(),
        })
        .commit(&shop.store)
        .await
        .unwrap();
    assert!(matters().await.1.unwrap() >= in_cart);
    shop.db
        .execute("UPDATE aliexpress_products SET wanted_at = 100", ())
        .await
        .unwrap();
    timada_aliexpress::note_wanted(&shop.db, &shop.tenant.id, &shop.product_id, 50)
        .await
        .unwrap();
    assert_eq!(matters().await.1, Some(100), "it only moves forward");

    // Off sale.
    timada_catalog::Command::new(shop.store.clone(), &shop.tenant.id)
        .unpublish_product(ALICE, &shop.product_id)
        .await
        .unwrap();
    assert!(!matters().await.0);
}

#[tokio::test]
async fn the_sweep_spends_the_day_on_what_shoppers_want_first() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    // Another shop's product on sale, tried long before this one.
    let on_sale = timada_aliexpress::LinkState {
        id: "other-item".to_owned(),
        product_id: Some("other-product".to_owned()),
        status: timada_aliexpress::LinkStatus::Linked,
        last_synced_at: Some(100),
        ..Default::default()
    };
    timada_aliexpress::upsert_link(&shop.db, "other-shop", &on_sale)
        .await
        .unwrap();
    timada_aliexpress::note_published(&shop.db, "other-shop", "other-product", true)
        .await
        .unwrap();
    shop.add_to_cart().await;
    shop.follow_read_models().await;

    let now = crate::auth::now();
    let later = now + shop.aliexpress.sync_every_secs() + 1;
    let due =
        timada_aliexpress::links_due(&shop.db, later as i64, now as i64 - 60, Share::ALONE, 10)
            .await
            .unwrap();
    let ids: Vec<&str> = due.iter().map(|link| link.id.as_str()).collect();
    assert_eq!(ids, [shop.link_id.as_str(), "other-item"]);

    // The other shop is not connected, so only this one's is asked for.
    let version = shop.version().await;
    assert_eq!(sweep.ask_due(later).await.unwrap(), 2);
    assert_eq!(shop.version().await, version + 1);
}

impl Shop {
    /// A paid order placed on AliExpress: its forward, in the read model.
    async fn placed_on_aliexpress(&self) -> String {
        evento::append("an-order")
            .tenant(&self.tenant.id)
            .original_version(0)
            .event(&timada_orders::aggregator::Placed {
                reference: "K7Q4-MZP2".to_owned(),
                cart_id: "a-cart".to_owned(),
                email: "ada@example.com".to_owned(),
                shipping: Default::default(),
                note: None,
                lines: Vec::new(),
                currency: "EUR".to_owned(),
                total_cents: 862,
                payment: Default::default(),
                supplier: Some(timada_aliexpress::ALIEXPRESS_SUPPLIER.to_owned()),
                placed_by: "storefront".to_owned(),
            })
            .event(&timada_orders::aggregator::Paid {
                payment_id: String::new(),
                amount_cents: 862,
                paid_by: "timada-dashboard".to_owned(),
            })
            .commit(&self.store)
            .await
            .unwrap();
        let forwards = timada_aliexpress::Command::new(self.store.clone(), &self.tenant.id);
        let forward_id = forwards
            .request_forward(ALIEXPRESS_ACTOR, "an-order", false)
            .await
            .unwrap();
        assert!(
            forwards
                .start_forward(ALIEXPRESS_ACTOR, &forward_id)
                .await
                .unwrap()
        );
        forwards
            .record_placed(
                ALIEXPRESS_ACTOR,
                &forward_id,
                timada_aliexpress::Placement {
                    ae_order_ids: vec!["8100000000000001".to_owned()],
                    items: Vec::new(),
                    pay_requested: true,
                    complete: true,
                },
            )
            .await
            .unwrap();
        self.follow_read_models().await;
        forward_id
    }

    /// The sweep's next pass over the open orders, a period after `at`.
    fn period_after(&self, at: u64) -> u64 {
        at + self.aliexpress.order_read_every_secs() + 1
    }
}

#[tokio::test]
async fn an_open_order_is_read_by_the_node_it_belongs_to() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let forward_id = shop.placed_on_aliexpress().await;
    let later = shop.period_after(crate::auth::now());

    // Not this node's: left to the other.
    *shop.share.lock().unwrap() = share_without(&forward_id);
    sweep.check_forwards(later).await.unwrap();
    assert_eq!(shop.stand_in.reads(), 0);

    // This node's: read, once.
    *shop.share.lock().unwrap() = Share::ALONE;
    sweep.check_forwards(later).await.unwrap();
    assert_eq!(shop.stand_in.reads(), 1);
}

#[tokio::test]
async fn an_open_order_is_read_once_a_period_when_no_push_comes() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    shop.placed_on_aliexpress().await;
    let now = crate::auth::now();

    // Just placed: however often the sweep passes, nothing to ask.
    sweep.check_forwards(now).await.unwrap();
    sweep.check_forwards(now + 3_600).await.unwrap();
    assert_eq!(shop.stand_in.reads(), 0);

    let later = shop.period_after(now);
    sweep.check_forwards(later).await.unwrap();
    assert_eq!(shop.stand_in.reads(), 1);
    sweep.check_forwards(later + 3_600).await.unwrap();
    assert_eq!(shop.stand_in.reads(), 1, "read: a period to go");

    sweep
        .check_forwards(shop.period_after(later))
        .await
        .unwrap();
    assert_eq!(shop.stand_in.reads(), 2);
}

#[tokio::test]
async fn a_push_saves_the_sweep_its_read() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let forward_id = shop.placed_on_aliexpress().await;
    let now = crate::auth::now();
    let pushed_at = shop.period_after(now) - 10;

    super::webhook::check(
        shop.aliexpress.clone(),
        &shop.db,
        shop.store.clone(),
        &shop.tenant.id,
        &forward_id,
        super::webhook::Said::Nothing,
        pushed_at,
    )
    .await
    .unwrap();
    assert_eq!(shop.stand_in.reads(), 1, "read at once");

    // The period it would have been read at comes and goes.
    sweep.check_forwards(shop.period_after(now)).await.unwrap();
    assert_eq!(shop.stand_in.reads(), 1);

    // A period after the push, it is read in case another one was lost.
    sweep
        .check_forwards(shop.period_after(pushed_at))
        .await
        .unwrap();
    assert_eq!(shop.stand_in.reads(), 2);
}

#[tokio::test]
async fn a_node_that_stopped_with_the_parcels_known_leaves_the_order_to_the_next_sweep() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let forward_id = shop.placed_on_aliexpress().await;
    let forwards = timada_aliexpress::Command::new(shop.store.clone(), &shop.tenant.id);

    // A read recorded the parcel, then its node stopped before fulfilling.
    let (_, version) = forwards
        .load_forward_versioned(&forward_id)
        .await
        .unwrap()
        .unwrap();
    forwards
        .record_checked(
            &forward_id,
            version,
            vec![timada_aliexpress::aggregator::forward::Checked {
                ae_order_id: "8100000000000001".to_owned(),
                status: "WAIT_BUYER_ACCEPT_GOODS".to_owned(),
                logistics_status: "SELLER_SEND_GOODS".to_owned(),
                tracking: vec!["LP00123456FR".to_owned()],
                closed: false,
            }],
            None,
        )
        .await
        .unwrap();
    shop.follow_read_models().await;

    // A sweep later, not a day: the shopper is not left waiting.
    let went_through = sweep
        .check_forwards(crate::auth::now() + SWEEP_EVERY.as_secs() + 1)
        .await
        .unwrap();

    assert_eq!(went_through, 1);
    let forward = forwards
        .load_forward_versioned(&forward_id)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(forward.status, timada_aliexpress::ForwardStatus::Done);
    let order = timada_orders::Command::new(shop.store.clone(), &shop.tenant.id)
        .load_order("an-order")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(order.tracking.as_deref(), Some("LP00123456FR"));
}

#[tokio::test]
async fn a_shop_that_cannot_read_takes_no_turn_with_its_open_orders() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let forward_id = shop.placed_on_aliexpress().await;
    let later = shop.period_after(crate::auth::now());
    timada_tenancy::Command(shop.store.clone())
        .remove_aliexpress(ALICE, &shop.tenant.id)
        .await
        .unwrap();

    sweep.check_forwards(later).await.unwrap();

    assert_eq!(shop.stand_in.reads(), 0);
    let before = later as i64 - 1;
    let due = timada_aliexpress::forwards_due(&shop.db, before, before, Share::ALONE, 10)
        .await
        .unwrap();
    assert!(
        due.iter().all(|forward| forward.id != forward_id),
        "passed over until the next period"
    );
}

/// A forward closed with its parcels, as the read model has it: what the
/// parcels sweep goes through.
fn shipped_forward(id: &str, closed_at: u64) -> timada_aliexpress::ForwardState {
    timada_aliexpress::ForwardState {
        id: id.to_owned(),
        order_id: format!("order-{id}"),
        status: timada_aliexpress::ForwardStatus::Done,
        ae_orders: vec![timada_aliexpress::AeOrderState {
            id: format!("81{id}"),
            tracking: vec![format!("LP{id}")],
            ..Default::default()
        }],
        complete: true,
        fulfilled_tracking: Some(format!("LP{id}")),
        updated_at: closed_at,
        ..Default::default()
    }
}

#[tokio::test]
async fn the_parcels_of_every_shop_take_turns_and_a_still_one_waits_longer() {
    let shop = Shop::new().await;
    let now = crate::auth::now();
    for (tenant, id) in [
        ("a-shop", "a1"),
        ("a-shop", "a2"),
        ("a-shop", "a3"),
        ("b-shop", "b1"),
    ] {
        timada_aliexpress::upsert_forward(&shop.db, tenant, &shipped_forward(id, now - 10))
            .await
            .unwrap();
    }
    // Asked about every hour while they move, every eight at most.
    let due = async |at: u64, limit: i64| -> Vec<String> {
        timada_aliexpress::parcels_due(
            &shop.db,
            at as i64,
            3_600,
            8 * 3_600,
            (at - timada_aliexpress::FORWARD_WATCH_SECS) as i64,
            Share::ALONE,
            limit,
        )
        .await
        .unwrap()
        .iter()
        .map(|forward| forward.id.clone())
        .collect()
    };

    // Never asked about: due, the shops taking turns.
    assert_eq!(due(now, 2).await, ["a1", "b1"]);
    assert_eq!(due(now, 10).await, ["a1", "a2", "a3", "b1"]);

    // A parcel that moved is asked about again a period later.
    timada_aliexpress::note_parcels_followed(&shop.db, "a-shop", "a1", now, true)
        .await
        .unwrap();
    assert!(!due(now + 3_599, 10).await.contains(&"a1".to_owned()));
    assert!(due(now + 3_600, 10).await.contains(&"a1".to_owned()));

    // One that stays still waits as long as it has been still: twice as
    // long each time.
    timada_aliexpress::note_parcels_followed(&shop.db, "a-shop", "a1", now + 3_600, false)
        .await
        .unwrap();
    assert!(!due(now + 7_199, 10).await.contains(&"a1".to_owned()));
    assert!(due(now + 7_200, 10).await.contains(&"a1".to_owned()));
    timada_aliexpress::note_parcels_followed(&shop.db, "a-shop", "a1", now + 7_200, false)
        .await
        .unwrap();
    assert!(!due(now + 14_399, 10).await.contains(&"a1".to_owned()));
    assert!(due(now + 14_400, 10).await.contains(&"a1".to_owned()));
    // Up to eight periods.
    timada_aliexpress::note_parcels_followed(&shop.db, "a-shop", "a1", now + 86_400, false)
        .await
        .unwrap();
    assert!(
        !due(now + 86_400 + 8 * 3_600 - 1, 10)
            .await
            .contains(&"a1".to_owned())
    );
    assert!(
        due(now + 86_400 + 8 * 3_600, 10)
            .await
            .contains(&"a1".to_owned())
    );

    // At the door: no more.
    timada_aliexpress::note_parcels_delivered(&shop.db, "a-shop", "a1", now + 100_000)
        .await
        .unwrap();
    assert!(!due(now + 10 * 86_400, 10).await.contains(&"a1".to_owned()));
    assert!(due(now + 10 * 86_400, 10).await.contains(&"a2".to_owned()));
}

#[tokio::test]
async fn a_confirmed_push_ends_the_following_of_the_parcels() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let forward_id = shop.placed_on_aliexpress().await;
    let forwards = timada_aliexpress::Command::new(shop.store.clone(), &shop.tenant.id);
    // The parcel is known, and the order fulfilled with it by the sweep.
    let (_, version) = forwards
        .load_forward_versioned(&forward_id)
        .await
        .unwrap()
        .unwrap();
    forwards
        .record_checked(
            &forward_id,
            version,
            vec![timada_aliexpress::aggregator::forward::Checked {
                ae_order_id: "8100000000000001".to_owned(),
                status: "WAIT_BUYER_ACCEPT_GOODS".to_owned(),
                logistics_status: "SELLER_SEND_GOODS".to_owned(),
                tracking: vec!["LP00123456FR".to_owned()],
                closed: false,
            }],
            None,
        )
        .await
        .unwrap();
    shop.follow_read_models().await;
    let fulfilled_at = crate::auth::now() + SWEEP_EVERY.as_secs() + 1;
    assert_eq!(sweep.check_forwards(fulfilled_at).await.unwrap(), 1);
    shop.follow_read_models().await;
    let reads = shop.stand_in.reads();

    // The sweep asks where the parcel is a period after the fulfilment did.
    assert_eq!(sweep.follow_parcels(fulfilled_at).await.unwrap(), 0);
    let later = fulfilled_at + shop.aliexpress.parcel_read_every_secs() + 1;
    assert_eq!(sweep.follow_parcels(later).await.unwrap(), 1);
    assert_eq!(shop.stand_in.reads(), reads + 1);

    // AliExpress says the buyer has the goods: asked about once more, for the
    // last steps, then no more.
    super::webhook::check(
        shop.aliexpress.clone(),
        &shop.db,
        shop.store.clone(),
        &shop.tenant.id,
        &forward_id,
        super::webhook::Said::Confirmed,
        later + 1,
    )
    .await
    .unwrap();
    assert_eq!(shop.stand_in.reads(), reads + 2);
    assert_eq!(
        sweep.follow_parcels(later + 10 * 86_400).await.unwrap(),
        0,
        "followed to the door"
    );
}

#[tokio::test]
async fn a_routine_read_asks_for_the_delivery_only_when_the_cost_moved() {
    let shop = Shop::new().await;
    let freights = || shop.stand_in.freights.load(Ordering::SeqCst);

    // The import did not go through this node's reads: the first read asks.
    shop.sync(SyncMode::Routine).await;
    assert_eq!(freights(), 1);
    // The same cost: what was read stands.
    shop.sync(SyncMode::Routine).await;
    assert_eq!(freights(), 1);

    // A sale asks every time, and so does a member.
    shop.sync(SyncMode::Sale).await;
    shop.sync(SyncMode::Asked { by: ALICE }).await;
    assert_eq!(freights(), 3);

    // The cost moved: asked again, and the fee read goes into the price.
    *shop.stand_in.cost.lock().unwrap() = "5.00".to_owned();
    shop.stand_in.delivery_cents.store(100, Ordering::SeqCst);
    shop.sync(SyncMode::Routine).await;
    assert_eq!(freights(), 4);
    assert_eq!(shop.rose().await.0, 1_200, "(5.00 + 1.00) × 2");
    shop.sync(SyncMode::Routine).await;
    assert_eq!(freights(), 4);
    assert_eq!(shop.rose().await.0, 1_200);
}

#[tokio::test]
async fn a_payment_reads_the_items_of_its_cart_and_the_price_follows() {
    let shop = Shop::new().await;
    shop.read_at(100).await;
    *shop.stand_in.cost.lock().unwrap() = "5.00".to_owned();

    // Twice the same product (two of its variants), and one not imported.
    shop.goods()
        .read_again(
            &shop.tenant.id,
            &[
                shop.product_id.clone(),
                "not-imported".to_owned(),
                shop.product_id.clone(),
            ],
        )
        .await;

    assert_eq!(shop.stand_in.reads(), 1);
    assert_eq!(shop.rose().await.0, 1000);
}

#[tokio::test]
async fn a_payment_does_not_read_an_item_read_a_moment_ago() {
    let shop = Shop::new().await;
    let now = crate::auth::now();
    shop.read_at(now - 30).await;

    shop.goods()
        .read_again(&shop.tenant.id, std::slice::from_ref(&shop.product_id))
        .await;
    assert_eq!(shop.stand_in.reads(), 0);

    // A read that changed nothing leaves its time, so the next one holds off.
    shop.read_at(now - 61).await;
    let goods = shop.goods();
    goods
        .read_again(&shop.tenant.id, std::slice::from_ref(&shop.product_id))
        .await;
    assert_eq!(shop.stand_in.reads(), 1);
    goods
        .read_again(&shop.tenant.id, std::slice::from_ref(&shop.product_id))
        .await;
    assert_eq!(shop.stand_in.reads(), 1);
}

#[tokio::test]
async fn a_payment_goes_by_the_last_read_when_aliexpress_refuses() {
    let shop = Shop::new().await;
    shop.read_at(100).await;
    shop.stand_in.down.store(true, Ordering::SeqCst);

    shop.goods()
        .read_again(&shop.tenant.id, std::slice::from_ref(&shop.product_id))
        .await;

    assert_eq!(shop.stand_in.reads(), 1);
    assert_eq!(shop.rose().await.0, 862, "the price stays");
    let row = timada_aliexpress::link_of_product(&shop.db, &shop.tenant.id, &shop.product_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.last_synced_at,
        Some(100),
        "a read that failed is no read"
    );
    assert!(row.last_tried_at > Some(100), "it was tried, though");
    assert!(
        shop.row().await.last_error.is_some(),
        "the merchant can read why"
    );
}

#[tokio::test]
async fn a_payment_waits_a_moment_for_aliexpress_and_the_next_ones_do_not_wait_at_all() {
    let shop = Shop::new().await;
    shop.read_at(100).await;
    shop.stand_in.hung.store(true, Ordering::SeqCst);
    let goods = shop.goods().waiting(std::time::Duration::from_millis(100));

    let started = std::time::Instant::now();
    goods
        .read_again(&shop.tenant.id, std::slice::from_ref(&shop.product_id))
        .await;
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(shop.stand_in.reads(), 1, "it asked, and got no answer");

    // AliExpress does not answer: the next payments go by the last read.
    goods
        .read_again(&shop.tenant.id, std::slice::from_ref(&shop.product_id))
        .await;
    assert_eq!(shop.stand_in.reads(), 1, "they do not even ask");
}

#[tokio::test]
async fn a_shop_without_its_account_goes_by_the_last_read() {
    let shop = Shop::new().await;
    shop.read_at(100).await;
    timada_tenancy::Command(shop.store.clone())
        .remove_aliexpress(ALICE, &shop.tenant.id)
        .await
        .unwrap();

    shop.goods()
        .read_again(&shop.tenant.id, std::slice::from_ref(&shop.product_id))
        .await;

    assert_eq!(shop.stand_in.reads(), 0);
}

#[tokio::test]
async fn a_cart_add_asks_for_an_item_not_tried_lately_once() {
    let shop = Shop::new().await;
    shop.follow_requests().await;
    shop.read_at(100).await;
    let version = shop.version().await;

    // Not this node's: left to the other.
    *shop.share.lock().unwrap() = shop.elsewhere();
    shop.add_to_cart().await;
    shop.follow_requests().await;
    assert_eq!(shop.version().await, version);
    assert_eq!(shop.stand_in.reads(), 0);

    // This node's: asked for, and the request read in turn.
    *shop.share.lock().unwrap() = Share::ALONE;
    shop.add_to_cart().await;
    shop.follow_requests().await;
    assert_eq!(shop.version().await, version + 1);
    let link = shop
        .links()
        .load_link_by_id(&shop.link_id)
        .await
        .unwrap()
        .unwrap();
    assert!(link.last_requested_at.is_some());
    shop.follow_requests().await;
    assert_eq!(shop.stand_in.reads(), 1);

    // Tried a moment ago: the next carts ask for nothing.
    shop.add_to_cart().await;
    shop.follow_requests().await;
    assert_eq!(shop.version().await, version + 1);
    assert_eq!(shop.stand_in.reads(), 1);
}

#[tokio::test]
async fn a_cart_add_of_a_product_not_imported_asks_for_nothing() {
    let shop = Shop::new().await;
    shop.follow_requests().await;
    let catalog = timada_catalog::Command::new(shop.store.clone(), &shop.tenant.id);
    let mug = catalog
        .create_product(
            ALICE,
            timada_catalog::CreateProduct {
                name: "Mug".into(),
                slug: None,
            },
        )
        .await
        .unwrap();
    catalog
        .price_product(ALICE, &mug.id, 1250, "EUR")
        .await
        .unwrap();
    catalog.publish_product(ALICE, &mug.id).await.unwrap();
    let orders = timada_orders::Command::new(shop.store.clone(), &shop.tenant.id);
    orders
        .start_cart(
            timada_orders::STOREFRONT_ACTOR,
            timada_orders::CartItem {
                product_id: mug.id.clone(),
                variant_id: None,
                quantity: 1,
            },
        )
        .await
        .unwrap();
    let version = shop.version().await;

    shop.follow_requests().await;

    assert_eq!(shop.version().await, version);
    assert_eq!(shop.stand_in.reads(), 0);
}

#[tokio::test]
async fn a_price_stays_within_the_tolerance_unless_a_member_asks() {
    let shop = Shop::new().await;
    let tenancy = timada_tenancy::Command(shop.store.clone());
    tenancy
        .configure_aliexpress(
            ALICE,
            &shop.tenant.id,
            ConfigureAliexpress {
                ship_to_country: "FR".into(),
                currency: "EUR".into(),
                language: "fr".into(),
                pricing: shop.tenant.aliexpress_pricing.clone(),
                price_tolerance_bps: 500,
            },
        )
        .await
        .unwrap();
    let shop = Shop {
        tenant: tenancy.load_tenant(&shop.tenant.id).await.unwrap().unwrap(),
        ..shop
    };
    // 4.40 gives 8.80, about 2 % above 8.62.
    *shop.stand_in.cost.lock().unwrap() = "4.40".to_owned();

    shop.sync(SyncMode::Routine).await;
    assert_eq!(shop.rose().await.0, 862);

    shop.sync(SyncMode::Asked { by: ALICE }).await;
    assert_eq!(shop.rose().await.0, 880);
}

/// A node renewed the company's access and recorded AliExpress's tokens on
/// its claim, then stopped before the company heard of them: the next sweep
/// applies them, without asking AliExpress again (the stand-in answers no
/// renewal, so asking would have failed).
#[tokio::test]
async fn recorded_tokens_reach_the_company_without_asking_again() {
    let shop = Shop::new().await;
    let now = crate::auth::now();
    let company_id = shop.tenant.company.as_ref().unwrap().company_id.clone();
    let tenancy = timada_tenancy::Command(shop.store.clone());
    let due = now + 3_600;
    tenancy
        .authorize_aliexpress(
            ALICE,
            &company_id,
            AuthorizeAliexpress {
                access_token_sealed: shop.aliexpress.seal(&company_id, "token").unwrap(),
                refresh_token_sealed: shop.aliexpress.seal(&company_id, "refresh").unwrap(),
                expires_at: due,
                refresh_expires_at: 0,
                account: "shop@example.org".into(),
            },
        )
        .await
        .unwrap();
    let renewed = timada_aliexpress::RenewedTokens {
        access_token_sealed: shop.aliexpress.seal(&company_id, "renewed").unwrap(),
        refresh_token_sealed: shop
            .aliexpress
            .seal(&company_id, "renewed-refresh")
            .unwrap(),
        expires_at: now + 30 * 86_400,
        refresh_expires_at: 0,
        account: "shop@example.org".into(),
    };
    assert_eq!(
        timada_aliexpress::claim_token_refresh(&shop.store, "node-gone", &company_id, due, now)
            .await
            .unwrap(),
        timada_aliexpress::TokenRenewal::Claimed
    );
    timada_aliexpress::record_token_renewal(&shop.store, "node-gone", &company_id, due, &renewed)
        .await
        .unwrap();

    let company = tenancy.load_company(&company_id).await.unwrap().unwrap();
    let company = shop.aliexpress.refresh_if_due(company, now).await.unwrap();
    let authorization = company.aliexpress.unwrap();
    assert_eq!(authorization.expires_at, renewed.expires_at);
    assert_eq!(
        authorization.access_token_sealed,
        renewed.access_token_sealed
    );
    // Told again, the access is no longer due: nothing changes.
    let again = tenancy.load_company(&company_id).await.unwrap().unwrap();
    let again = shop.aliexpress.refresh_if_due(again, now).await.unwrap();
    assert_eq!(again.aliexpress.unwrap().expires_at, renewed.expires_at);
}

/// The store of a node whose next `fails` writes of the event `name` of the
/// aggregate `aggregate_type` are refused, as while writes are paused;
/// everything else goes through.
#[derive(Clone)]
struct RefusesWrites {
    store: Evento,
    aggregate_type: &'static str,
    name: &'static str,
    fails: Arc<AtomicUsize>,
}

impl RefusesWrites {
    /// Refuses the next `fails` records of what AliExpress took.
    fn placed(store: &Evento, fails: usize) -> Self {
        Self::new(store, "timada-aliexpress/Forward", "Placed", fails)
    }

    fn new(store: &Evento, aggregate_type: &'static str, name: &'static str, fails: usize) -> Self {
        Self {
            store: store.clone(),
            aggregate_type,
            name,
            fails: Arc::new(AtomicUsize::new(fails)),
        }
    }
}

#[async_trait::async_trait]
impl evento::Executor for RefusesWrites {
    async fn write(&self, events: Vec<evento::Event>) -> Result<(), evento::WriteError> {
        let matched = events
            .iter()
            .any(|event| event.aggregate_type == self.aggregate_type && event.name == self.name);
        let refused = matched
            && self
                .fails
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok();
        if refused {
            return Err(evento::WriteError::Unavailable(anyhow::anyhow!(
                "writes paused"
            )));
        }
        self.store.write(events).await
    }

    async fn read(
        &self,
        aggregators: Option<Arc<[evento::EventFilter]>>,
        routing_key: Option<evento::RoutingKey>,
        tenant: evento::TenantFilter,
        args: evento::cursor::Args,
        to_micros: Option<u64>,
    ) -> anyhow::Result<evento::cursor::ReadResult<evento::Event>> {
        self.store
            .read(aggregators, routing_key, tenant, args, to_micros)
            .await
    }

    async fn latest_timestamp(
        &self,
        aggregators: Option<Arc<[evento::EventFilter]>>,
        routing_key: Option<evento::RoutingKey>,
        tenant: evento::TenantFilter,
    ) -> anyhow::Result<u64> {
        self.store
            .latest_timestamp(aggregators, routing_key, tenant)
            .await
    }

    async fn stream_routing_key(
        &self,
        tenant: String,
        aggregate_type: String,
        aggregate_id: String,
    ) -> anyhow::Result<Option<Option<String>>> {
        self.store
            .stream_routing_key(tenant, aggregate_type, aggregate_id)
            .await
    }

    async fn get_subscriber_cursor(
        &self,
        key: String,
    ) -> anyhow::Result<Option<evento::cursor::Value>> {
        self.store.get_subscriber_cursor(key).await
    }

    async fn is_subscriber_running(
        &self,
        key: String,
        worker_id: ulid::Ulid,
    ) -> anyhow::Result<bool> {
        self.store.is_subscriber_running(key, worker_id).await
    }

    async fn upsert_subscriber(&self, key: String, worker_id: ulid::Ulid) -> anyhow::Result<()> {
        self.store.upsert_subscriber(key, worker_id).await
    }

    async fn acknowledge(
        &self,
        key: String,
        worker_id: ulid::Ulid,
        cursor: evento::cursor::Value,
        lag: u64,
    ) -> anyhow::Result<bool> {
        self.store.acknowledge(key, worker_id, cursor, lag).await
    }

    async fn get_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
    ) -> anyhow::Result<Option<(Vec<u8>, evento::cursor::Value)>> {
        self.store
            .get_snapshot(tenant, aggregate_type, projection, aggregate_revision, id)
            .await
    }

    async fn save_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        aggregate_revision: String,
        id: String,
        data: Vec<u8>,
        cursor: evento::cursor::Value,
    ) -> anyhow::Result<()> {
        self.store
            .save_snapshot(
                tenant,
                aggregate_type,
                projection,
                aggregate_revision,
                id,
                data,
                cursor,
            )
            .await
    }

    async fn delete_snapshot(
        &self,
        tenant: String,
        aggregate_type: String,
        projection: String,
        id: String,
    ) -> anyhow::Result<()> {
        self.store
            .delete_snapshot(tenant, aggregate_type, projection, id)
            .await
    }
}

impl Shop {
    /// A paid order of the item's first combination, shipped where AliExpress
    /// delivers to a phone it can call, and its placing asked for: the
    /// forward.
    async fn order_to_place(&self) -> String {
        self.order_to_place_with(true).await
    }

    /// The same, its money taken (`paid`) or only held.
    async fn order_to_place_with(&self, paid: bool) -> String {
        let product = timada_catalog::Command::new(self.store.clone(), &self.tenant.id)
            .load_product(&self.product_id)
            .await
            .unwrap()
            .unwrap();
        let mut write = evento::append("an-order");
        write.tenant(&self.tenant.id).original_version(0).event(
            &timada_orders::aggregator::Placed {
                reference: "K7Q4-MZP2".to_owned(),
                cart_id: "a-cart".to_owned(),
                email: "ada@example.com".to_owned(),
                shipping: timada_orders::ShippingAddress {
                    name: "Ada Lovelace".into(),
                    line1: "1 rue de la Paix".into(),
                    postal_code: "75002".into(),
                    city: "Paris".into(),
                    country: "FR".into(),
                    phone: Some("06 12 34 56 78".into()),
                    ..Default::default()
                },
                note: None,
                lines: vec![timada_orders::OrderLine {
                    product_id: self.product_id.clone(),
                    variant_id: Some(product.variants[0].id.clone()),
                    quantity: 1,
                    ..Default::default()
                }],
                currency: "EUR".to_owned(),
                total_cents: 862,
                payment: self.held_payment("a-cart", 862),
                supplier: Some(timada_aliexpress::ALIEXPRESS_SUPPLIER.to_owned()),
                placed_by: "storefront".to_owned(),
            },
        );
        if paid {
            write.event(&timada_orders::aggregator::Paid {
                payment_id: String::new(),
                amount_cents: 862,
                paid_by: "timada-dashboard".to_owned(),
            });
        }
        write.commit(&self.store).await.unwrap();
        self.links()
            .request_forward(ALIEXPRESS_ACTOR, "an-order", false)
            .await
            .unwrap()
    }

    /// A payment the checkout opened for `cart_id` and the shopper
    /// confirmed, as the provider holds it and as the order names it.
    fn held_payment(&self, cart_id: &str, amount_cents: u64) -> Payment {
        let payment_id = format!("pi_{cart_id}");
        self.provider
            .hold(&payment_id, &self.tenant.id, cart_id, amount_cents);
        Payment {
            provider: PROVIDER_STRIPE.to_owned(),
            payment_id,
            amount_cents,
            currency: "EUR".to_owned(),
            capture_before: None,
        }
    }

    fn orders(&self) -> timada_orders::Command<Evento> {
        timada_orders::Command::new(self.store.clone(), &self.tenant.id)
    }

    async fn order(&self) -> timada_orders::OrderState {
        self.orders().load_order("an-order").await.unwrap().unwrap()
    }

    /// Places the forward, by a node writing to `store`.
    async fn forward_on<E: evento::Executor + Clone>(
        &self,
        store: &E,
        forward_id: &str,
    ) -> super::forward::ForwardResult {
        self.aliexpress
            .forward_order(store, &self.db, &self.payments, &self.tenant.id, forward_id)
            .await
            .unwrap()
    }

    async fn forward(&self, forward_id: &str) -> timada_aliexpress::ForwardState {
        self.links()
            .load_forward_by_id(forward_id)
            .await
            .unwrap()
            .unwrap()
    }

    fn placed(&self) -> usize {
        self.stand_in.placed.load(Ordering::SeqCst)
    }

    /// A push from AliExpress of `status` for `ae_order_id`, signed with the
    /// app's key and secret, to the platform.
    async fn push(&self, ae_order_id: &str, status: &str) -> super::webhook::Outcome {
        let body = serde_json::json!({
            "data": { "buyerId": 1861703697, "orderId": ae_order_id, "orderStatus": status },
            "message_type": 53,
            "seller_id": "1234",
            "site": "ae_global",
            "timestamp": 1719389200,
        })
        .to_string();
        let signature = timada_aliexpress::push_signature("key", "secret", body.as_bytes());
        super::webhook::receive(
            &self.aliexpress,
            &self.store,
            body.as_bytes(),
            Some(&signature),
        )
        .await
        .unwrap()
    }

    /// The node's subscription catches up with the pushes, and the reads it
    /// started end.
    async fn follow_pushes(&self) {
        super::webhook::pushes_subscription::<Evento>(
            self.aliexpress.clone(),
            self.db.clone(),
            self.store.clone(),
        )
        .run_once(&self.store)
        .await
        .unwrap();
        self.aliexpress.pushes_read().await;
    }

    fn forward_ref(&self, forward_id: &str) -> timada_aliexpress::ForwardRef {
        timada_aliexpress::ForwardRef {
            tenant_id: self.tenant.id.clone(),
            forward_id: forward_id.to_owned(),
        }
    }
}

/// AliExpress took the order while the store refused the record for a
/// moment: it is recorded once writes come back, by the same run, and its
/// AliExpress order then names the forward for the pushes.
#[tokio::test]
async fn an_order_aliexpress_took_is_recorded_once_the_store_takes_writes_again() {
    let shop = Shop::new().await;
    let forward_id = shop.order_to_place().await;
    let paused = RefusesWrites::placed(&shop.store, 2);

    let placed = shop.forward_on(&paused, &forward_id).await;

    assert_eq!(
        placed,
        super::forward::ForwardResult::Placed(vec![PLACED_AE_ORDER.to_owned()])
    );
    assert_eq!(shop.placed(), 1, "placed once");
    let forward = shop.forward(&forward_id).await;
    assert_eq!(forward.status, timada_aliexpress::ForwardStatus::Placed);
    assert_eq!(forward.ae_orders[0].id, PLACED_AE_ORDER);
    let ae_order = timada_aliexpress::load_ae_order(&shop.store, PLACED_AE_ORDER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ae_order.forward, Some(shop.forward_ref(&forward_id)));
}

/// AliExpress took the order and the store refused its record for longer
/// than placing waits (a node rejoining, say): what AliExpress took is kept
/// on this node, and the sweep records it once the store takes writes again,
/// however long that is, even after the forward was given up as stalled.
/// Nothing places it twice meanwhile.
#[tokio::test]
async fn an_order_aliexpress_took_is_recorded_by_the_sweep_when_the_store_refused_it() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let forward_id = shop.order_to_place().await;
    let paused = RefusesWrites::placed(&shop.store, usize::MAX);

    assert_eq!(
        shop.forward_on(&paused, &forward_id).await,
        super::forward::ForwardResult::Unrecorded(vec![PLACED_AE_ORDER.to_owned()])
    );
    assert_eq!(shop.placed(), 1);
    let forward = shop.forward(&forward_id).await;
    assert_eq!(forward.status, timada_aliexpress::ForwardStatus::Started);
    let kept = timada_aliexpress::unrecorded(&shop.db, 10).await.unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].forward_id, forward_id);
    assert_eq!(
        kept[0].placement().unwrap().ae_order_ids,
        [PLACED_AE_ORDER.to_owned()]
    );
    // Asked again, it is not placed twice.
    assert_eq!(
        shop.forward_on(&shop.store, &forward_id).await,
        super::forward::ForwardResult::Skipped
    );
    assert_eq!(shop.placed(), 1);

    // Given up as stalled by the node it belongs to while the store still
    // refused the record here.
    shop.follow_read_models().await;
    let stalled = forward.updated_at + timada_aliexpress::FORWARD_STALL_SECS + 1;
    assert_eq!(sweep.settle_stalled(stalled).await.unwrap(), 1);
    assert_eq!(
        shop.forward(&forward_id).await.last_error.as_deref(),
        Some(super::forward::MAY_HAVE_PLACED)
    );

    // The store takes writes: placed after all, its capture to follow.
    assert_eq!(sweep.record_unrecorded().await.unwrap(), 1);
    let forward = shop.forward(&forward_id).await;
    assert_eq!(forward.status, timada_aliexpress::ForwardStatus::Placed);
    assert_eq!(forward.ae_orders[0].id, PLACED_AE_ORDER);
    assert!(
        timada_aliexpress::unrecorded(&shop.db, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(sweep.record_unrecorded().await.unwrap(), 0);
    assert_eq!(shop.placed(), 1);
}

/// An attempt that never said how it went may be on AliExpress: placed
/// again and surely refused, the order keeps its fence, so the shopper still
/// cannot cancel it and a dropped hold leaves it open. Once the merchant
/// says it is not there, the fence goes.
#[tokio::test]
async fn a_refusal_after_an_attempt_that_may_have_placed_keeps_the_fence() {
    let shop = Shop::new().await;
    let forward_id = shop.order_to_place_with(false).await;
    assert!(shop.order().await.cancellable_by_shopper());

    shop.stand_in
        .place_answers
        .lock()
        .unwrap()
        .extend([PlaceAnswer::Lost, PlaceAnswer::Refused]);
    assert_eq!(
        shop.forward_on(&shop.store, &forward_id).await,
        super::forward::ForwardResult::Failed(super::forward::MAY_HAVE_PLACED.to_owned())
    );
    let order = shop.order().await;
    assert!(order.supplier_uncertain);
    assert!(!order.cancellable_by_shopper());

    // Placed again: surely refused, the first may be there all the same.
    shop.links()
        .request_forward(ALICE, "an-order", false)
        .await
        .unwrap();
    let refused = shop.forward_on(&shop.store, &forward_id).await;
    assert!(
        matches!(&refused, super::forward::ForwardResult::Failed(reason) if reason.starts_with("AliExpress refused the order")),
        "{refused:?}"
    );
    assert_eq!(shop.placed(), 2);
    let order = shop.order().await;
    assert_eq!(order.supplier_order, timada_orders::SupplierOrder::Started);
    assert!(!order.cancellable_by_shopper());

    // The merchant looked: it is not there. The shopper may cancel again.
    shop.orders()
        .rule_out_supplier_order(ALICE, "an-order", timada_aliexpress::ALIEXPRESS_SUPPLIER)
        .await
        .unwrap();
    assert!(shop.order().await.cancellable_by_shopper());
}

/// A node that stopped while placing leaves the forward taken on: once it
/// stalled, the sweep tells the order its supplier may have it before saying
/// so on the forward, so a later attempt AliExpress refuses keeps the fence.
#[tokio::test]
async fn a_stalled_attempt_keeps_the_fence_against_a_later_refusal() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let forward_id = shop.order_to_place_with(false).await;
    // What a node writes before asking AliExpress, then it stopped.
    let forwards = shop.links();
    assert!(
        forwards
            .start_forward(ALIEXPRESS_ACTOR, &forward_id)
            .await
            .unwrap()
    );
    let (_, version) = forwards
        .load_forward_versioned(&forward_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        forwards
            .begin_asking(ALIEXPRESS_ACTOR, &forward_id, version)
            .await
            .unwrap()
    );
    shop.orders()
        .begin_supplier_order(
            ALIEXPRESS_ACTOR,
            "an-order",
            timada_aliexpress::ALIEXPRESS_SUPPLIER,
            crate::auth::now(),
        )
        .await
        .unwrap();
    shop.follow_read_models().await;

    let stalled =
        shop.forward(&forward_id).await.updated_at + timada_aliexpress::FORWARD_STALL_SECS + 1;
    assert_eq!(sweep.settle_stalled(stalled).await.unwrap(), 1);
    assert!(shop.order().await.supplier_uncertain);

    shop.stand_in
        .place_answers
        .lock()
        .unwrap()
        .push_back(PlaceAnswer::Refused);
    shop.links()
        .request_forward(ALICE, "an-order", false)
        .await
        .unwrap();
    shop.forward_on(&shop.store, &forward_id).await;
    assert_eq!(
        shop.placed(),
        1,
        "only the second attempt reached AliExpress here"
    );
    let order = shop.order().await;
    assert_eq!(order.supplier_order, timada_orders::SupplierOrder::Started);
    assert!(!order.cancellable_by_shopper());
}

/// The merchant places a stuck order again before the sweep came round: the
/// attempt before never said how it went, so the order counts it uncertain
/// as the new one begins, and the new one's sure refusal keeps the fence.
#[tokio::test]
async fn an_order_placed_again_before_the_sweep_keeps_the_fence() {
    let shop = Shop::new().await;
    let forward_id = shop.order_to_place_with(false).await;
    // What a node writes before asking AliExpress, then it stopped.
    let forwards = shop.links();
    assert!(
        forwards
            .start_forward(ALIEXPRESS_ACTOR, &forward_id)
            .await
            .unwrap()
    );
    let (_, version) = forwards
        .load_forward_versioned(&forward_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        forwards
            .begin_asking(ALIEXPRESS_ACTOR, &forward_id, version)
            .await
            .unwrap()
    );
    shop.orders()
        .begin_supplier_order(
            ALIEXPRESS_ACTOR,
            "an-order",
            timada_aliexpress::ALIEXPRESS_SUPPLIER,
            crate::auth::now(),
        )
        .await
        .unwrap();

    shop.links()
        .request_forward(ALICE, "an-order", true)
        .await
        .unwrap();
    shop.stand_in
        .place_answers
        .lock()
        .unwrap()
        .push_back(PlaceAnswer::Refused);
    let refused = shop.forward_on(&shop.store, &forward_id).await;
    assert!(
        matches!(&refused, super::forward::ForwardResult::Failed(reason) if reason.starts_with("AliExpress refused the order")),
        "{refused:?}"
    );
    let order = shop.order().await;
    assert!(order.supplier_uncertain);
    assert_eq!(order.supplier_order, timada_orders::SupplierOrder::Started);
    assert!(!order.cancellable_by_shopper());
}

/// AliExpress refused the order and the store would not tell the order so:
/// tried again, then left taken on, so the stall sweep tells the order its
/// supplier may have it, rather than an attempt staying open on a failed
/// forward for good.
#[tokio::test]
async fn a_refusal_the_order_never_hears_of_is_left_to_the_stall_sweep() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(
        shop.db.clone(),
        shop.store.clone(),
        shop.aliexpress.clone(),
        SWEEP_EVERY,
    );
    let forward_id = shop.order_to_place_with(false).await;
    shop.stand_in
        .place_answers
        .lock()
        .unwrap()
        .push_back(PlaceAnswer::Refused);
    let paused = RefusesWrites::new(
        &shop.store,
        "timada-orders/Order",
        "SupplierOrderRefused",
        usize::MAX,
    );

    let placed = shop
        .aliexpress
        .forward_order(
            &paused,
            &shop.db,
            &shop.payments,
            &shop.tenant.id,
            &forward_id,
        )
        .await;
    assert!(placed.is_err(), "{placed:?}");
    let forward = shop.forward(&forward_id).await;
    assert_eq!(forward.status, timada_aliexpress::ForwardStatus::Started);
    assert!(shop.order().await.supplier_attempt_open);

    shop.follow_read_models().await;
    let stalled = forward.updated_at + timada_aliexpress::FORWARD_STALL_SECS + 1;
    assert_eq!(sweep.settle_stalled(stalled).await.unwrap(), 1);
    let order = shop.order().await;
    assert!(order.supplier_uncertain);
    assert!(!order.supplier_attempt_open);
    assert_eq!(
        shop.forward(&forward_id).await.last_error.as_deref(),
        Some(super::forward::MAY_HAVE_PLACED)
    );
}

/// An attempt overtaken while it read what it needs (the merchant asked
/// again, or the sweep gave it up) asks AliExpress nothing: the forward's
/// fence, written at the version it took the forward on at, refuses it.
#[tokio::test]
async fn an_attempt_overtaken_before_asking_places_nothing() {
    let shop = Arc::new(Shop::new().await);
    let forward_id = shop.order_to_place().await;
    shop.stand_in.held.store(true, Ordering::SeqCst);
    let placing = tokio::spawn({
        let shop = shop.clone();
        let forward_id = forward_id.clone();
        async move { shop.forward_on(&shop.store, &forward_id).await }
    });
    shop.stand_in.holding.notified().await;

    // The merchant asks again while the first attempt reads the product.
    shop.links()
        .request_forward(ALICE, "an-order", true)
        .await
        .unwrap();
    shop.stand_in.held.store(false, Ordering::SeqCst);
    shop.stand_in.answer.notify_one();

    assert_eq!(
        placing.await.unwrap(),
        super::forward::ForwardResult::Skipped
    );
    assert_eq!(shop.placed(), 0);
    let order = shop.order().await;
    assert_eq!(
        order.supplier_order,
        timada_orders::SupplierOrder::None,
        "the order was not fenced for nothing"
    );
    assert_eq!(
        shop.forward(&forward_id).await.status,
        timada_aliexpress::ForwardStatus::Requested
    );
}

/// A push the store does not record is not answered as taken: AliExpress
/// sends it again.
#[tokio::test]
async fn a_push_the_store_does_not_record_is_an_error() {
    let shop = Shop::new().await;
    let paused = RefusesWrites::new(&shop.store, "timada-aliexpress/AeOrder", "Pushed", 1);
    let body = br#"{"message_type":53,"data":{"orderId":"8100000000000001","orderStatus":"OrderShipped"}}"#;
    let signature = timada_aliexpress::push_signature("key", "secret", body);

    let refused = super::webhook::receive(&shop.aliexpress, &paused, body, Some(&signature)).await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(
        timada_aliexpress::load_ae_order(&shop.store, "8100000000000001")
            .await
            .unwrap()
            .is_none()
    );

    // Sent again, recorded.
    assert_eq!(
        super::webhook::receive(&shop.aliexpress, &paused, body, Some(&signature))
            .await
            .unwrap(),
        super::webhook::Outcome::Accepted { forward: None }
    );
}

/// A push is recorded before it is answered, nothing read in the request;
/// the subscription reads the order, once, and a push that does not check
/// out records nothing.
#[tokio::test]
async fn a_push_is_recorded_then_read_by_the_subscription() {
    let shop = Shop::new().await;
    let forward_id = shop.placed_on_aliexpress().await;

    assert_eq!(
        shop.push("8100000000000001", "OrderShipped").await,
        super::webhook::Outcome::Accepted {
            forward: Some(shop.forward_ref(&forward_id))
        }
    );
    assert_eq!(shop.stand_in.reads(), 0, "answered before any read");
    shop.follow_pushes().await;
    assert_eq!(shop.stand_in.reads(), 1);
    shop.follow_pushes().await;
    assert_eq!(shop.stand_in.reads(), 1, "read once");

    let body = br#"{"message_type":53,"data":{"orderId":"8100000000000001","orderStatus":"OrderShipped"}}"#;
    let forged = super::webhook::receive(&shop.aliexpress, &shop.store, body, Some("00"))
        .await
        .unwrap();
    assert_eq!(forged, super::webhook::Outcome::Rejected);
    let other = br#"{"message_type":1,"data":{"trade_order_id":"3016"}}"#;
    let signature = timada_aliexpress::push_signature("key", "secret", other);
    assert_eq!(
        super::webhook::receive(&shop.aliexpress, &shop.store, other, Some(&signature))
            .await
            .unwrap(),
        super::webhook::Outcome::Ignored
    );
    let order = timada_aliexpress::load_ae_order(&shop.store, "8100000000000001")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(order.waiting, 0);
    shop.follow_pushes().await;
    assert_eq!(shop.stand_in.reads(), 1);
}

/// A push that comes before the forward recorded its order (AliExpress
/// pushing as it answers) waits in the order's stream: the forward's record
/// reads the order then, and a push after it is read at once.
#[tokio::test]
async fn a_push_before_the_order_is_recorded_is_read_once_it_is() {
    let shop = Shop::new().await;
    let forward_id = shop.order_to_place().await;

    assert_eq!(
        shop.push(PLACED_AE_ORDER, "OrderCreated").await,
        super::webhook::Outcome::Accepted { forward: None }
    );
    shop.follow_pushes().await;
    assert_eq!(shop.stand_in.reads(), 0, "no forward to read yet");

    shop.aliexpress
        .forward_order(
            &shop.store,
            &shop.db,
            &shop.payments,
            &shop.tenant.id,
            &forward_id,
        )
        .await
        .unwrap();
    let reads = shop.stand_in.reads();
    shop.follow_read_models().await;
    shop.follow_pushes().await;
    assert_eq!(
        shop.stand_in.reads(),
        reads + 1,
        "read for the push that waited"
    );

    assert_eq!(
        shop.push(PLACED_AE_ORDER, "OrderShipped").await,
        super::webhook::Outcome::Accepted {
            forward: Some(shop.forward_ref(&forward_id))
        }
    );
    shop.follow_pushes().await;
    assert_eq!(shop.stand_in.reads(), reads + 2);
}

/// The node an order belongs to reads it after a push; the others leave it.
#[tokio::test]
async fn a_push_is_read_by_the_node_its_order_belongs_to() {
    let shop = Shop::new().await;
    let forward_id = shop.placed_on_aliexpress().await;

    *shop.share.lock().unwrap() = share_without(&forward_id);
    shop.push("8100000000000001", "OrderShipped").await;
    shop.follow_pushes().await;
    assert_eq!(shop.stand_in.reads(), 0);

    *shop.share.lock().unwrap() = Share::ALONE;
    shop.push("8100000000000001", "OrderConfirmed").await;
    shop.follow_pushes().await;
    assert_eq!(shop.stand_in.reads(), 1);
}

/// A push saying AliExpress closed the order stands while the order cannot
/// be read: the forward closes, and the shop's order is to be cancelled.
#[tokio::test]
async fn a_closing_push_closes_the_order_through_the_subscription() {
    let shop = Shop::new().await;
    let forward_id = shop.placed_on_aliexpress().await;
    shop.stand_in.down.store(true, Ordering::SeqCst);

    shop.push("8100000000000001", timada_aliexpress::ORDER_CLOSED)
        .await;
    shop.follow_pushes().await;

    assert!(shop.forward(&forward_id).await.all_closed());
    let order = timada_orders::Command::new(shop.store.clone(), &shop.tenant.id)
        .load_order("an-order")
        .await
        .unwrap()
        .unwrap();
    let asked = order
        .pending_cancellation
        .expect("its cancellation is asked");
    assert_eq!(asked.reason, super::forward::CLOSED_BY_ALIEXPRESS);
}

/// An order a `store:write` key forged in the shop's store names no payment
/// the provider knows as the shop's: nothing is bought for it on the
/// company's account (timada #431). The provider not answering refuses the
/// forward too, to try again.
#[tokio::test]
async fn an_order_the_checkout_did_not_place_is_not_placed_on_aliexpress() {
    let shop = Shop::new().await;
    let product = timada_catalog::Command::new(shop.store.clone(), &shop.tenant.id)
        .load_product(&shop.product_id)
        .await
        .unwrap()
        .unwrap();
    let forged = async |order_id: &str, payment: Payment| {
        evento::append(order_id)
            .tenant(&shop.tenant.id)
            .original_version(0)
            .event(&timada_orders::aggregator::Placed {
                reference: "K7Q4-MZP2".to_owned(),
                cart_id: "a-cart".to_owned(),
                email: "anyone@example.com".to_owned(),
                shipping: timada_orders::ShippingAddress {
                    name: "Ada Lovelace".into(),
                    line1: "1 rue de la Paix".into(),
                    postal_code: "75002".into(),
                    city: "Paris".into(),
                    country: "FR".into(),
                    ..Default::default()
                },
                note: None,
                lines: vec![timada_orders::OrderLine {
                    product_id: shop.product_id.clone(),
                    variant_id: Some(product.variants[0].id.clone()),
                    quantity: 1,
                    ..Default::default()
                }],
                currency: "EUR".to_owned(),
                total_cents: 862,
                payment,
                supplier: Some(timada_aliexpress::ALIEXPRESS_SUPPLIER.to_owned()),
                placed_by: "storefront".to_owned(),
            })
            .commit(&shop.store)
            .await
            .unwrap();
        shop.links()
            .request_forward(ALIEXPRESS_ACTOR, order_id, false)
            .await
            .unwrap()
    };

    // A payment the provider never opened.
    let forward_id = forged(
        "forged-order",
        Payment {
            provider: PROVIDER_STRIPE.to_owned(),
            payment_id: "pi_forged".to_owned(),
            amount_cents: 862,
            currency: "EUR".to_owned(),
            capture_before: None,
        },
    )
    .await;
    let placed = shop.forward_on(&shop.store, &forward_id).await;
    let super::forward::ForwardResult::Failed(reason) = placed else {
        panic!("{placed:?}");
    };
    assert!(
        reason.contains("not placed through the shop's checkout"),
        "{reason}"
    );
    assert_eq!(shop.placed(), 0);
    let forward = shop.forward(&forward_id).await;
    assert_eq!(forward.status, timada_aliexpress::ForwardStatus::Failed);
    assert_eq!(forward.last_error.as_deref(), Some(reason.as_str()));

    // One the provider opened for another cart of the shop.
    let other = shop.held_payment("another-cart", 862);
    let forward_id = forged("borrowed-order", other).await;
    let placed = shop.forward_on(&shop.store, &forward_id).await;
    let super::forward::ForwardResult::Failed(reason) = placed else {
        panic!("{placed:?}");
    };
    assert!(
        reason.contains("not placed through the shop's checkout"),
        "{reason}"
    );
    assert_eq!(shop.placed(), 0);

    // The provider down: not refused for good.
    let forward_id = shop.order_to_place().await;
    shop.provider.set_down(Some("connection reset"));
    let placed = shop.forward_on(&shop.store, &forward_id).await;
    let super::forward::ForwardResult::Failed(reason) = placed else {
        panic!("{placed:?}");
    };
    assert!(reason.contains("did not answer"), "{reason}");
    assert_eq!(shop.placed(), 0);

    // Up again, the merchant asks again: placed.
    shop.provider.set_down(None);
    let forward_id = shop
        .links()
        .request_forward(ALICE, "an-order", true)
        .await
        .unwrap();
    assert!(matches!(
        shop.forward_on(&shop.store, &forward_id).await,
        super::forward::ForwardResult::Placed(_)
    ));
    assert_eq!(shop.placed(), 1);
}
