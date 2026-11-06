//! Reads of linked items against a stand-in for AliExpress, on a store of
//! their own: what a read writes, which node reads, what the sweep asks for.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use evento::Evento;
use timada_aliexpress::aggregator::SkuLink;
use timada_aliexpress::{AeProduct, Share, map_product, track_imported};
use timada_catalog_import::{ImportOptions, NoImages, import_one};
use timada_tenancy::{
    AuthorizeAliexpress, ConfigureAliexpress, CreateTenant, LegalIdentity, Pricing, PricingRule,
    Rounding, TenantState,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

use super::payment_reads::AliexpressGoods;
use super::subscription::reads_subscription;
use super::sync::AliexpressSync;
use super::{ALIEXPRESS_ACTOR, Aliexpress, SyncMode, SyncResult};
use crate::config::AliexpressConfig;
use crate::db::Db;
use crate::payments::SuppliedGoods;
use crate::seal::Sealer;

const AE_PRODUCT: &str = "1005006123456789";
const ALICE: &str = "alice";

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
    /// What delivering one costs, in cents; free by default.
    delivery_cents: AtomicU64,
    /// The orders placed, each as [`PLACED_AE_ORDER`].
    placed: AtomicUsize,
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
            delivery_cents: AtomicU64::new(0),
            placed: AtomicUsize::new(0),
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
                        let body = serde_json::json!({ "aliexpress_ds_order_create_response": {
                            "result": { "is_success": true, "order_list": { "number": [PLACED_AE_ORDER] } },
                        } })
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
                calls_per_day: 86_400_000,
                read_fresh_secs: 60,
            },
            false,
            store.clone(),
            Arc::new(move || *current.lock().unwrap()),
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

    /// Sets when the item was last read, in this node's read model.
    async fn read_at(&self, at: u64) {
        self.db
            .execute(
                "UPDATE aliexpress_products SET last_synced_at = ?1, last_tried_at = ?1",
                (at as i64,),
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
        let cart_id = orders
            .create_cart(timada_orders::STOREFRONT_ACTOR)
            .await
            .unwrap();
        orders
            .add_to_cart(
                timada_orders::STOREFRONT_ACTOR,
                &cart_id,
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
    shop.db
        .execute(
            "UPDATE aliexpress_products SET last_synced_at = 100, last_tried_at = 100",
            (),
        )
        .await
        .unwrap();
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
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
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
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
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
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
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
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
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
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
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
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
    let forward_id = shop.placed_on_aliexpress().await;
    let now = crate::auth::now();
    let pushed_at = shop.period_after(now) - 10;

    super::webhook::check(
        shop.aliexpress.clone(),
        &shop.db,
        shop.store.clone(),
        &shop.tenant.id,
        &forward_id,
        None,
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
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
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

    // A minute later, not a day: the shopper is not left waiting.
    let fulfilled = sweep.check_forwards(crate::auth::now() + 60).await.unwrap();

    assert_eq!(fulfilled, 1);
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
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
    let forward_id = shop.placed_on_aliexpress().await;
    let later = shop.period_after(crate::auth::now());
    timada_tenancy::Command(shop.store.clone())
        .remove_aliexpress(ALICE, &shop.tenant.id)
        .await
        .unwrap();

    sweep.check_forwards(later).await.unwrap();

    assert_eq!(shop.stand_in.reads(), 0);
    let before = later as i64 - 1;
    let due = timada_aliexpress::forwards_due(&shop.db, before, Share::ALONE, 10)
        .await
        .unwrap();
    assert!(
        due.iter().all(|forward| forward.id != forward_id),
        "passed over until the next period"
    );
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
    let cart_id = orders
        .create_cart(timada_orders::STOREFRONT_ACTOR)
        .await
        .unwrap();
    orders
        .add_to_cart(
            timada_orders::STOREFRONT_ACTOR,
            &cart_id,
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

/// The store of a node whose next `fails` writes of a forward's `Placed` are
/// refused, as while writes are paused; everything else goes through.
#[derive(Clone)]
struct RefusesPlaced {
    store: Evento,
    fails: Arc<AtomicUsize>,
}

impl RefusesPlaced {
    fn new(store: &Evento, fails: usize) -> Self {
        Self {
            store: store.clone(),
            fails: Arc::new(AtomicUsize::new(fails)),
        }
    }
}

#[async_trait::async_trait]
impl evento::Executor for RefusesPlaced {
    async fn write(&self, events: Vec<evento::Event>) -> Result<(), evento::WriteError> {
        let placed = events.iter().any(|event| {
            event.aggregate_type == "timada-aliexpress/Forward" && event.name == "Placed"
        });
        let refused = placed
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
        let product = timada_catalog::Command::new(self.store.clone(), &self.tenant.id)
            .load_product(&self.product_id)
            .await
            .unwrap()
            .unwrap();
        evento::append("an-order")
            .tenant(&self.tenant.id)
            .original_version(0)
            .event(&timada_orders::aggregator::Placed {
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
        self.links()
            .request_forward(ALIEXPRESS_ACTOR, "an-order", false)
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

    /// The node's subscription catches up with the pushes.
    async fn follow_pushes(&self) {
        super::webhook::pushes_subscription::<Evento>(
            self.aliexpress.clone(),
            self.db.clone(),
            self.store.clone(),
        )
        .run_once(&self.store)
        .await
        .unwrap();
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
    let paused = RefusesPlaced::new(&shop.store, 2);

    let placed = shop
        .aliexpress
        .forward_order(&paused, &shop.db, &shop.tenant.id, &forward_id)
        .await
        .unwrap();

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

/// AliExpress took the order and the store never recorded it: the orders
/// are logged, the forward stays taken on, and once it stalled the sweep
/// says AliExpress may have it. Nothing places it again on its own, and the
/// shopper can no longer cancel it.
#[tokio::test]
async fn an_order_aliexpress_took_and_never_recorded_is_left_to_the_merchant() {
    let shop = Shop::new().await;
    let sweep = AliexpressSync::new(shop.db.clone(), shop.store.clone(), shop.aliexpress.clone());
    let forward_id = shop.order_to_place().await;
    let paused = RefusesPlaced::new(&shop.store, usize::MAX);

    let placed = shop
        .aliexpress
        .forward_order(&paused, &shop.db, &shop.tenant.id, &forward_id)
        .await
        .unwrap();

    assert_eq!(
        placed,
        super::forward::ForwardResult::Unrecorded(vec![PLACED_AE_ORDER.to_owned()])
    );
    assert_eq!(shop.placed(), 1);
    let forward = shop.forward(&forward_id).await;
    assert_eq!(forward.status, timada_aliexpress::ForwardStatus::Started);
    // Asked again, it is not placed twice.
    assert_eq!(
        shop.aliexpress
            .forward_order(&shop.store, &shop.db, &shop.tenant.id, &forward_id)
            .await
            .unwrap(),
        super::forward::ForwardResult::Skipped
    );
    assert_eq!(shop.placed(), 1);

    shop.follow_read_models().await;
    let now = crate::auth::now();
    assert_eq!(sweep.settle_stalled(now).await.unwrap(), 0, "being placed");
    let stalled = forward.updated_at + timada_aliexpress::FORWARD_STALL_SECS + 1;
    assert_eq!(sweep.settle_stalled(stalled).await.unwrap(), 1);
    let forward = shop.forward(&forward_id).await;
    assert_eq!(forward.status, timada_aliexpress::ForwardStatus::Failed);
    assert_eq!(
        forward.last_error.as_deref(),
        Some(super::forward::MAY_HAVE_PLACED)
    );
    assert_eq!(sweep.settle_stalled(stalled).await.unwrap(), 0, "said once");
    let order = timada_orders::Command::new(shop.store.clone(), &shop.tenant.id)
        .load_order("an-order")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(order.supplier_order, timada_orders::SupplierOrder::Started);
    assert_eq!(shop.placed(), 1);
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
        .forward_order(&shop.store, &shop.db, &shop.tenant.id, &forward_id)
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
