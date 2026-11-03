//! Reads of linked items against a stand-in for AliExpress, on a store of
//! their own: what a read writes, which node reads, what the sweep asks for.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use evento::Evento;
use timada_aliexpress::{AeProduct, Share, map_product, track_imported};
use timada_catalog_import::{ImportOptions, NoImages, import_one};
use timada_tenancy::{
    AuthorizeAliexpress, ConfigureAliexpress, CreateTenant, LegalIdentity, Pricing, PricingRule,
    Rounding, TenantState,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::subscription::reads_subscription;
use super::sync::AliexpressSync;
use super::{ALIEXPRESS_ACTOR, Aliexpress, SyncMode, SyncResult};
use crate::config::AliexpressConfig;
use crate::db::Db;
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
    reads: AtomicUsize,
}

impl StandIn {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cost: Mutex::new("4.31".to_owned()),
            stock: AtomicU32::new(120),
            offline: AtomicBool::new(false),
            down: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
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
                let mut request = Vec::new();
                let mut chunk = [0; 4096];
                while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                self.reads.fetch_add(1, Ordering::SeqCst);
                let answer = if self.down.load(Ordering::SeqCst) {
                    "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
                } else {
                    let body = self.product().to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
        });
        url
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
    async fn sync(&self, mode: SyncMode) -> SyncResult {
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

    /// A node of two that does not hold the item.
    fn elsewhere(&self) -> Share {
        let holder = (0..2)
            .find(|&index| Share { index, of: 2 }.owns(&self.link_id))
            .unwrap();
        Share {
            index: 1 - holder,
            of: 2,
        }
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
async fn a_read_a_member_waits_for_records_its_outcome_even_when_nothing_changed() {
    let shop = Shop::new().await;
    let version = shop.version().await;

    shop.sync(SyncMode::Asked).await;

    assert_eq!(shop.version().await, version + 1);
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
    shop.sync(SyncMode::Asked).await;
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

    assert_eq!(sweep.sweep(now).await.unwrap(), 0, "just imported");

    let later = now + shop.aliexpress.sync_every_secs() + 1;
    let version = shop.version().await;
    assert_eq!(sweep.sweep(later).await.unwrap(), 1);
    assert_eq!(shop.version().await, version + 1);
    assert_eq!(sweep.sweep(later).await.unwrap(), 0, "asked already");
    assert_eq!(shop.version().await, version + 1);

    // A node holding another share leaves it alone.
    *shop.share.lock().unwrap() = shop.elsewhere();
    let after = later + shop.aliexpress.sync_every_secs() + 1;
    assert_eq!(sweep.sweep(after).await.unwrap(), 0);
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

    assert_eq!(sweep.sweep(later).await.unwrap(), 0);

    assert_eq!(shop.version().await, version, "no request");
    assert_eq!(shop.row().await.last_tried_at, Some(later as i64));
    assert_eq!(shop.stand_in.reads(), 0);
}
