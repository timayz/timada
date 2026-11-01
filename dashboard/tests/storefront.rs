//! The headless storefront library against the store API on one node: a
//! `Store` follows its tenant's catalog into a local database, keeps its
//! cursor there and nowhere else, resumes from it after a restart, is cut
//! off with its key, connects over TLS, and carries the category tree the
//! products are filed under. Selling goes through the dashboard's
//! `/store/carts/*` side, stood in for here over a socket, since the key
//! reads neither carts nor orders through the store API.
//!
//! The dashboard is a binary crate, so the modules are reached through the
//! binary's own test harness (`#[path]` includes), which drags in items this
//! test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/db.rs"]
mod db;
#[path = "../src/peers.rs"]
mod peers;
#[path = "../src/rate_limit.rs"]
mod rate_limit;
#[path = "../src/read_only_store.rs"]
mod read_only_store;
#[path = "../src/store_api.rs"]
mod store_api;
#[path = "../src/store_carts.rs"]
mod store_carts;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use evento::remote::ServerHandle;
use serde_json::{Value, json};
use timada_catalog::{
    AddImage, CreateCategory, CreateProduct, DefineVariant, OptionSelection, categories_of_tenant,
};
use timada_core::db::Db;
use timada_storefront::{
    CATALOG_SUBSCRIPTION, ProductQuery, ProductSort, Store, StoreConfig, StoreTls,
    SubscriptionState, category_ancestors, find_category, menu_of, published_categories,
    published_category_body, published_category_by_slug, published_page, published_product_by_slug,
    published_product_categories, published_products, published_related_products,
    search_published_pages,
};
use timada_tenancy::{
    Command, CreateApiKey, CreateTenant, SCOPE_STORE_CHECKOUT, SCOPE_STORE_READ, TenantState,
    read_model_subscription,
};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use topcoat::router::StatusCode;

use cluster::Cluster;
use config::StoreApiConfig;
use store_api::StoreConnections;

const NOW: u64 = 1_800_000_000;

struct Harness {
    cluster: Arc<Cluster>,
    platform: Command<cluster::Executor>,
    /// The store's file, read directly for what the node recorded in it.
    store: db::Store,
    /// The read models in the store's database.
    db: db::Db,
    dir: PathBuf,
    connections: Arc<StoreConnections>,
    addr: SocketAddr,
    _server: ServerHandle,
}

/// A self-signed `localhost` certificate as PEM files in `dir`, and a client
/// connector trusting exactly it.
fn tls_files(dir: &std::path::Path) -> (PathBuf, PathBuf, TlsConnector) {
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap();
    let cert_path = dir.join("store-api.crt");
    let key_path = dir.join("store-api.key");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    let mut roots = RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let client = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (cert_path, key_path, TlsConnector::from(Arc::new(client)))
}

async fn harness(tls: Option<(PathBuf, PathBuf)>) -> Harness {
    let dir = std::env::temp_dir().join(format!("timada-storefront-{}", ulid::Ulid::generate()));
    std::fs::create_dir_all(&dir).unwrap();
    // The store in memory (a file would fsync every commit of the thirty-odd
    // tests running at once), the read models in it.
    let files = cluster::Files {
        path: PathBuf::from(":memory:"),
    };
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    db::migrate_store_read_models(&store).await.unwrap();
    let store_db = db::Db::from(store.clone());
    let (cluster, platform) = Cluster::open(None, None, &files, store.clone())
        .await
        .unwrap();
    let connections = StoreConnections::new();
    let (tls_cert, tls_key) = match tls {
        Some((cert, key)) => (Some(cert), Some(key)),
        None => (None, None),
    };
    let config = StoreApiConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        public_addr: None,
        tls_cert,
        tls_key,
    };
    // The ephemeral port is only known from the listener; bind here and
    // hand the real address to the config the module reads.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let config = StoreApiConfig {
        listen: addr,
        ..config
    };
    let server = store_api::serve(Some(&config), platform.clone(), connections.clone())
        .await
        .unwrap()
        .unwrap();
    Harness {
        cluster,
        platform: Command(platform),
        store,
        db: store_db,
        dir,
        connections,
        addr,
        _server: server,
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
        read_model_subscription::<cluster::Executor>(self.db.clone())
            .run_once(&self.platform.0)
            .await
            .unwrap();
        tenant
    }

    async fn key(&self, tenant: &TenantState) -> (String, String) {
        self.key_with(tenant, &[SCOPE_STORE_READ]).await
    }

    async fn key_with(&self, tenant: &TenantState, scopes: &[&str]) -> (String, String) {
        let created = self
            .platform
            .create_api_key(
                "alice",
                CreateApiKey {
                    tenant_id: tenant.id.clone(),
                    label: "Shop".into(),
                    scopes: scopes.iter().map(|s| s.to_string()).collect(),
                },
                NOW,
            )
            .await
            .unwrap();
        (created.token, created.key.id)
    }

    /// A local database for one storefront replica, in memory: every clone
    /// of it is the same database, as a file's would be.
    async fn local_db(&self, _name: &str) -> Db {
        Db::from(evento::Turso::in_memory_concurrent().await.unwrap())
    }

    fn config(&self, token: &str) -> StoreConfig {
        StoreConfig::new(self.addr, token).request_timeout(Duration::from_secs(2))
    }

    /// The catalog gateway in the tenant's streams, as the dashboard pages
    /// use it.
    fn catalog(&self, tenant: &TenantState) -> timada_catalog::Command<cluster::Executor> {
        timada_catalog::Command::new(self.platform.clone(), &tenant.id)
    }

    /// The pages gateway in the tenant's streams.
    fn pages(&self, tenant: &TenantState) -> timada_pages::Command<cluster::Executor> {
        timada_pages::Command::new(self.platform.clone(), &tenant.id)
    }

    /// The subscription cursors recorded in the store.
    async fn store_cursors(&self) -> Vec<String> {
        let conn = self.store.connect().unwrap();
        let mut rows = conn
            .query("SELECT key FROM subscriber ORDER BY key", ())
            .await
            .unwrap();
        let mut keys = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            keys.push(row.get::<String>(0).unwrap());
        }
        keys
    }

    /// How many storefronts of the tenant are connected.
    fn connected(&self, tenant: &TenantState) -> usize {
        self.connections.count_tenant(&tenant.id)
    }

    async fn wait_for(&self, what: &str, mut ok: impl FnMut(&Self) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !ok(self) {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn finish(self) {
        self._server.shutdown().await;
        self.cluster.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn local_cursors(db: &Db) -> Vec<String> {
    db.fetch_all::<(String,)>("SELECT key FROM subscriber ORDER BY key", ())
        .await
        .unwrap()
        .into_iter()
        .map(|(key,)| key)
        .collect()
}

/// Polls `check` until it holds or ten seconds pass.
async fn eventually<F>(what: &str, mut check: F)
where
    F: AsyncFnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn published(db: &Db) -> Vec<String> {
    published_products(db, &ProductQuery::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|p| p.name)
        .collect()
}

#[tokio::test]
async fn a_store_follows_its_tenant_catalog_locally() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let zeta = h.create("zeta").await;
    let (token, _) = h.key(&acme).await;

    // Acme has one published product and one draft; zeta has its own.
    let catalog = h.catalog(&acme);
    let mug = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "Blue Mug".into(),
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
    catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "Draft Plate".into(),
                slug: None,
            },
        )
        .await
        .unwrap();
    h.catalog(&zeta)
        .create_product(
            "bob",
            CreateProduct {
                name: "Zeta Thing".into(),
                slug: None,
            },
        )
        .await
        .unwrap();

    let db = h.local_db("acme-shop").await;
    let store = Store::connect(h.config(&token), db.clone()).await.unwrap();
    eventually("the published product is in the local copy", async || {
        published(&db).await == vec!["Blue Mug".to_owned()]
    })
    .await;
    let found = published_product_by_slug(&db, "blue-mug")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.price().as_deref(), Some("12.50 €"));
    assert!(
        published_product_by_slug(&db, "draft-plate")
            .await
            .unwrap()
            .is_none(),
        "a draft is not for sale"
    );
    assert!(
        published_product_by_slug(&db, "zeta-thing")
            .await
            .unwrap()
            .is_none(),
        "another tenant's catalog never shows"
    );

    // A change on the store reaches the copy by push, well under the poll
    // interval.
    catalog
        .rename_product("alice", &mug.id, "Big Blue Mug")
        .await
        .unwrap();
    let started = std::time::Instant::now();
    eventually("the rename reaches the local copy", async || {
        published(&db).await == vec!["Big Blue Mug".to_owned()]
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // The cursor is local; the store has no cursor of ours.
    assert_eq!(
        local_cursors(&db).await,
        vec![CATALOG_SUBSCRIPTION.to_owned()]
    );
    assert!(
        !h.store_cursors()
            .await
            .iter()
            .any(|key| key.contains(CATALOG_SUBSCRIPTION)),
        "{:?}",
        h.store_cursors().await
    );
    let status = store.status().await;
    assert!(status.connected);
    assert_eq!(status.subscription, SubscriptionState::Running);
    assert_eq!(status.products, 1);
    assert!(status.last_event_at.is_some());
    assert_eq!(h.connected(&acme), 1, "the connection is registered");

    store.shutdown().await;
    h.wait_for("the connection is released", |h| h.connected(&acme) == 0)
        .await;
    h.finish().await;
}

#[tokio::test]
async fn a_restarted_store_resumes_from_its_local_cursor() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme);
    let first = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "First".into(),
                slug: None,
            },
        )
        .await
        .unwrap();
    catalog
        .price_product("alice", &first.id, 100, "EUR")
        .await
        .unwrap();
    catalog.publish_product("alice", &first.id).await.unwrap();

    let db = h.local_db("replica").await;
    let store = Store::connect(h.config(&token), db.clone()).await.unwrap();
    eventually("the first product arrives", async || {
        published(&db).await.len() == 1
    })
    .await;
    let before = store.status().await.last_event_at;
    store.shutdown().await;

    // While the replica is down, the catalog moves on.
    let second = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "Second".into(),
                slug: None,
            },
        )
        .await
        .unwrap();
    catalog
        .price_product("alice", &second.id, 200, "EUR")
        .await
        .unwrap();
    catalog.publish_product("alice", &second.id).await.unwrap();

    let store = Store::connect(h.config(&token), db.clone()).await.unwrap();
    eventually("the second product arrives after the restart", async || {
        published(&db).await.len() == 2
    })
    .await;
    assert_eq!(local_cursors(&db).await.len(), 1, "one cursor, resumed");
    assert!(store.status().await.last_event_at >= before);

    store.shutdown().await;
    h.finish().await;
}

#[tokio::test]
async fn a_revoked_key_disconnects_the_store() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, key_id) = h.key(&acme).await;
    let db = h.local_db("revoked").await;
    let store = Store::connect(h.config(&token), db.clone()).await.unwrap();
    assert!(store.status().await.connected);

    h.platform
        .revoke_api_key("alice", &key_id, NOW + 1)
        .await
        .unwrap();
    assert_eq!(h.connections.close_key(&key_id), 1);
    eventually("the store reports the lost connection", async || {
        !store.status().await.connected
    })
    .await;
    let err = match Store::connect(h.config(&token), h.local_db("again").await).await {
        Ok(_) => panic!("a revoked key must be refused"),
        Err(err) => format!("{err:#}"),
    };
    assert!(err.contains("unauthorized"), "{err}");

    store.shutdown().await;
    h.finish().await;
}

#[tokio::test]
async fn the_store_connects_over_tls() {
    let dir =
        std::env::temp_dir().join(format!("timada-storefront-tls-{}", ulid::Ulid::generate()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert, key, _connector) = tls_files(&dir);
    let h = harness(Some((cert.clone(), key))).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme);
    let mug = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "Secure Mug".into(),
                slug: None,
            },
        )
        .await
        .unwrap();
    catalog
        .price_product("alice", &mug.id, 999, "USD")
        .await
        .unwrap();
    catalog.publish_product("alice", &mug.id).await.unwrap();

    let plain = Store::connect(h.config(&token), h.local_db("plain").await).await;
    assert!(
        plain.is_err(),
        "plain TCP against a TLS listener is refused"
    );

    let db = h.local_db("tls").await;
    let tls = StoreTls::from_ca_pem(&cert, "localhost").unwrap();
    let store = Store::connect(h.config(&token).tls(tls), db.clone())
        .await
        .unwrap();
    eventually("the product arrives over TLS", async || {
        published(&db).await == vec!["Secure Mug".to_owned()]
    })
    .await;

    store.shutdown().await;
    h.finish().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// A published product of `name` priced at `amount_cents`, filed under every
/// category of `categories`.
async fn stock(
    catalog: &timada_catalog::Command<cluster::Executor>,
    name: &str,
    amount_cents: u64,
    categories: &[&str],
) -> String {
    let product = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: name.to_owned(),
                slug: None,
            },
        )
        .await
        .unwrap();
    catalog
        .price_product("alice", &product.id, amount_cents, "EUR")
        .await
        .unwrap();
    for category in categories {
        catalog
            .categorize_product("alice", &product.id, category)
            .await
            .unwrap();
    }
    catalog.publish_product("alice", &product.id).await.unwrap();
    product.id
}

#[tokio::test]
async fn a_store_follows_the_category_tree_its_products_are_filed_under() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme);

    // Bedding > Mats, and Toys beside it.
    let bedding = catalog
        .create_category(
            "alice",
            CreateCategory {
                name: "Bedding".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mats = catalog
        .create_category(
            "alice",
            CreateCategory {
                name: "Mats".into(),
                slug: None,
                parent_id: Some(bedding.id.clone()),
            },
        )
        .await
        .unwrap();
    let toys = catalog
        .create_category(
            "alice",
            CreateCategory {
                name: "Toys".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    catalog
        .position_category("alice", &toys.id, 1)
        .await
        .unwrap();
    let cooling = stock(&catalog, "Cooling Mat", 2990, &[&mats.id]).await;
    let wool = stock(&catalog, "Wool Mat", 1990, &[&mats.id]).await;
    stock(&catalog, "Feather Wand", 990, &[&toys.id]).await;

    let db = h.local_db("acme-shop").await;
    let store = Store::connect(h.config(&token), db.clone()).await.unwrap();
    eventually(
        "every published product reaches the local copy",
        async || published(&db).await.len() == 3,
    )
    .await;
    eventually("and so does the category tree", async || {
        published_categories(&db).await.unwrap().len() == 2
    })
    .await;

    // The tree keeps its order and counts a subtree, not just one level.
    let tree = published_categories(&db).await.unwrap();
    assert_eq!(
        tree.iter()
            .map(|n| (n.category.name.as_str(), n.product_count))
            .collect::<Vec<_>>(),
        [("Bedding", 2), ("Toys", 1)]
    );
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].category.name, "Mats");
    assert_eq!(tree[0].children[0].product_count, 2);
    assert_eq!(
        find_category(&tree, "mats").map(|n| n.product_count),
        Some(2)
    );

    // A breadcrumb reads top-level first.
    let mats_row = published_category_by_slug(&db, "mats")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        category_ancestors(&tree, &mats_row)
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Bedding"]
    );

    // The text below a category's products reaches the store, apart from
    // the trees every page loads.
    catalog
        .write_category_body("alice", &mats.id, "How to pick a mat.")
        .await
        .unwrap();
    eventually("the body reaches the store", async || {
        published_category_body(&db, &mats.id).await.unwrap() == "How to pick a mat."
    })
    .await;
    assert_eq!(published_category_body(&db, &toys.id).await.unwrap(), "");

    // A collection page asks for a subtree, in the order a shopper picked.
    let cheapest_first = published_products(
        &db,
        &ProductQuery::default()
            .in_category(bedding.id.clone())
            .sort(ProductSort::PriceAsc),
    )
    .await
    .unwrap();
    assert_eq!(
        cheapest_first
            .items
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Wool Mat", "Cooling Mat"]
    );
    assert_eq!(cheapest_first.total, 2, "the toy is in another tree");
    let dearest_first = published_products(
        &db,
        &ProductQuery::default()
            .in_category(bedding.id.clone())
            .sort(ProductSort::PriceDesc),
    )
    .await
    .unwrap();
    assert_eq!(dearest_first.items[0].name, "Cooling Mat");
    let searched = published_products(
        &db,
        &ProductQuery::default()
            .in_category(bedding.id.clone())
            .search("wool"),
    )
    .await
    .unwrap();
    assert_eq!(searched.total, 1, "a search inside a category is counted");
    assert_eq!(searched.items[0].name, "Wool Mat");

    // A new price re-sorts a category's page though nothing was filed anew.
    catalog
        .price_product("alice", &wool, 3990, "EUR")
        .await
        .unwrap();
    eventually("the category page sorts by the new price", async || {
        published_products(
            &db,
            &ProductQuery::default()
                .in_category(bedding.id.clone())
                .sort(ProductSort::PriceAsc),
        )
        .await
        .unwrap()
        .items
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>()
            == ["Cooling Mat", "Wool Mat"]
    })
    .await;

    // Related products share a category and never include the product itself.
    let cooling_row = published_product_by_slug(&db, "cooling-mat")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        published_related_products(&db, &cooling_row, 4)
            .await
            .unwrap()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Wool Mat"]
    );
    assert_eq!(
        published_product_categories(&db, &cooling_row.id)
            .await
            .unwrap()
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Mats"]
    );

    // Hiding Bedding from the menu takes Mats along; both pages stay.
    catalog
        .set_category_in_menu("alice", &bedding.id, false)
        .await
        .unwrap();
    eventually("a hidden category leaves the menu", async || {
        menu_of(&published_categories(&db).await.unwrap())
            .iter()
            .map(|n| n.category.name.clone())
            .collect::<Vec<_>>()
            == ["Toys"]
    })
    .await;
    let tree = published_categories(&db).await.unwrap();
    assert_eq!(tree.len(), 2, "the tree still holds it");
    assert!(find_category(&tree, "mats").is_some());
    assert!(
        published_category_by_slug(&db, "bedding")
            .await
            .unwrap()
            .is_some_and(|c| !c.in_menu),
        "its page is still found"
    );
    catalog
        .set_category_in_menu("alice", &bedding.id, true)
        .await
        .unwrap();
    eventually("and comes back", async || {
        menu_of(&published_categories(&db).await.unwrap()).len() == 2
    })
    .await;

    // Leaving a category and archiving one both reach the local copy.
    catalog
        .uncategorize_product("alice", &cooling, &mats.id)
        .await
        .unwrap();
    eventually("a product that left is no longer counted", async || {
        find_category(&published_categories(&db).await.unwrap(), "mats").map(|n| n.product_count)
            == Some(1)
    })
    .await;
    catalog.archive_category("alice", &toys.id).await.unwrap();
    eventually("an archived category leaves the tree", async || {
        published_categories(&db).await.unwrap().len() == 1
    })
    .await;
    assert!(
        published_category_by_slug(&db, "toys")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        published(&db).await.len(),
        3,
        "its products are still for sale"
    );

    store.shutdown().await;
}

#[tokio::test]
async fn a_store_carries_the_gallery_with_the_address_it_is_loaded_from() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme);
    let mat = stock(&catalog, "Cooling Mat", 2990, &[]).await;
    for (id, alt) in [
        ("img-front", "Seen from the front"),
        ("img-cat", "A cat on it"),
    ] {
        catalog
            .add_image(
                "alice",
                &mat,
                AddImage {
                    image_id: id.into(),
                    ext: "png".into(),
                    size: 4096,
                    width: Some(1200),
                    height: Some(800),
                    alt: alt.into(),
                },
            )
            .await
            .unwrap();
    }

    // The host says where its pictures are served from; the library turns
    // each one into an address a browser can load.
    let db = h.local_db("acme-shop").await;
    let config = h
        .config(&token)
        .media_url("https://cdn.example/media/acme/");
    let store = Store::connect(config, db.clone()).await.unwrap();
    eventually("the product reaches the local copy", async || {
        published_product_by_slug(&db, "cooling-mat")
            .await
            .unwrap()
            .is_some_and(|p| p.images.len() == 2)
    })
    .await;

    let mut product = published_product_by_slug(&db, "cooling-mat")
        .await
        .unwrap()
        .unwrap();
    // The rows hold the file name; the address is added when a page reads it.
    assert_eq!(product.images[0].url, "");
    product.fill_urls(store.media_url());
    assert_eq!(product.images[0].file, "img-front.png");
    assert_eq!(
        product.images[0].url, "https://cdn.example/media/acme/img-front.png",
        "the trailing slash of the configured address is dropped"
    );
    assert_eq!(product.images[0].alt, "Seen from the front");
    assert_eq!(product.images[0].mime, "image/png");
    assert_eq!(
        (product.images[0].width, product.images[0].height),
        (Some(1200), Some(800))
    );
    assert_eq!(
        product.image().map(|i| i.id.as_str()),
        Some("img-front"),
        "a listing shows the first one"
    );

    // Reordering and removing reach the copy too.
    catalog
        .reorder_images("alice", &mat, vec!["img-cat".into(), "img-front".into()])
        .await
        .unwrap();
    eventually("the new order reaches the local copy", async || {
        published_product_by_slug(&db, "cooling-mat")
            .await
            .unwrap()
            .and_then(|p| p.image().map(|i| i.id.clone()))
            == Some("img-cat".to_owned())
    })
    .await;
    catalog
        .remove_image("alice", &mat, "img-cat")
        .await
        .unwrap();
    eventually("and so does a removal", async || {
        published_product_by_slug(&db, "cooling-mat")
            .await
            .unwrap()
            .is_some_and(|p| p.images.len() == 1)
    })
    .await;

    store.shutdown().await;
}

#[tokio::test]
async fn a_store_carries_the_variants_a_shopper_picks_from() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme);

    // A harness in two colours and two sizes, priced per combination, and a
    // plain product beside it.
    let harness_id = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "Cat Harness".into(),
                slug: None,
            },
        )
        .await
        .unwrap()
        .id;
    let colour = catalog
        .define_option(
            "alice",
            &harness_id,
            "Colour",
            vec!["Beige".into(), "Blue".into()],
        )
        .await
        .unwrap();
    let size = catalog
        .define_option("alice", &harness_id, "Size", vec!["S".into(), "M".into()])
        .await
        .unwrap();
    for (c, s, amount, sku) in [
        ("Beige", "S", 2290u64, "harness-beige-s"),
        ("Beige", "M", 2490, "harness-beige-m"),
        ("Blue", "S", 2290, "harness-blue-s"),
    ] {
        catalog
            .define_variant(
                "alice",
                &harness_id,
                DefineVariant {
                    selections: vec![
                        OptionSelection {
                            option_id: colour.clone(),
                            value: c.into(),
                        },
                        OptionSelection {
                            option_id: size.clone(),
                            value: s.into(),
                        },
                    ],
                    sku: Some(sku.into()),
                    amount_cents: amount,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap();
    }
    catalog
        .set_attribute("alice", &harness_id, "Material", "Nylon")
        .await
        .unwrap();
    // No price of its own: the variants are its price.
    catalog.publish_product("alice", &harness_id).await.unwrap();
    stock(&catalog, "Feather Wand", 990, &[]).await;

    let db = h.local_db("acme-shop").await;
    let store = Store::connect(h.config(&token), db.clone()).await.unwrap();
    eventually("the varying product reaches the local copy", async || {
        published_product_by_slug(&db, "cat-harness")
            .await
            .unwrap()
            .is_some_and(|p| p.variants.len() == 3)
    })
    .await;
    eventually("and so does the plain one beside it", async || {
        published(&db).await.len() == 2
    })
    .await;

    let harness = published_product_by_slug(&db, "cat-harness")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        harness
            .options
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        ["Colour", "Size"]
    );
    assert_eq!(harness.options[0].values, ["Beige", "Blue"]);
    // A listing shows what it may cost, not one of the amounts.
    assert_eq!(harness.price().as_deref(), Some("22.90–24.90 €"));
    let range = harness.price_range().unwrap();
    assert!(range.varies());
    assert_eq!((range.min_cents, range.max_cents), (2290, 2490));
    assert_eq!(
        harness.attributes,
        vec![timada_storefront::Attribute {
            name: "Material".into(),
            value: "Nylon".into(),
        }]
    );

    // The menus of a product page resolve to one variant, in any order.
    let picked = vec![
        (size.clone(), "M".to_owned()),
        (colour.clone(), "Beige".to_owned()),
    ];
    let variant = harness.variant(&picked).expect("that combination is sold");
    assert_eq!(variant.price(), "24.90 €");
    assert_eq!(variant.sku.as_deref(), Some("harness-beige-m"));
    assert_eq!(variant.value_of(&colour), Some("Beige"));
    assert!(
        harness
            .variant(&[(colour.clone(), "Blue".into()), (size.clone(), "M".into())])
            .is_none(),
        "the one combination they did not price is not sold"
    );

    // Sorting goes by what a shopper would pay, not by the base price.
    let cheapest = published_products(&db, &ProductQuery::default().sort(ProductSort::PriceAsc))
        .await
        .unwrap();
    assert_eq!(
        cheapest
            .items
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Feather Wand", "Cat Harness"]
    );
    let dearest = published_products(&db, &ProductQuery::default().sort(ProductSort::PriceDesc))
        .await
        .unwrap();
    assert_eq!(dearest.items[0].name, "Cat Harness");

    // A variant that goes narrows the range.
    catalog
        .remove_variant("alice", &harness_id, &harness.variants[1].id)
        .await
        .unwrap();
    eventually("the narrower range reaches the local copy", async || {
        published_product_by_slug(&db, "cat-harness")
            .await
            .unwrap()
            .and_then(|p| p.price())
            == Some("22.90 €".to_owned())
    })
    .await;

    store.shutdown().await;
}

/// Answers the next request to pay of a cart as the dashboard does: a
/// payment for it, or the refusal it gets; returns the payment opened.
async fn answer_as_dashboard(
    merchant: &timada_orders::Command<cluster::Executor>,
    cart_id: &str,
) -> Option<timada_orders::Payment> {
    let mut request = None;
    eventually("the shopper asks to pay", async || {
        let cart = merchant.load_cart(cart_id).await.unwrap().unwrap();
        request = cart
            .requests
            .iter()
            .find(|r| r.answer.is_none())
            .map(|r| r.request_id.clone());
        request.is_some()
    })
    .await;
    let request_id = request.unwrap();
    match merchant.pending_payment(cart_id, &request_id).await {
        Ok(Some(pending)) => {
            let payment = timada_orders::Payment {
                provider: "stripe".into(),
                payment_id: format!("pi_{request_id}"),
                amount_cents: pending.total_cents,
                currency: pending.currency.clone(),
            };
            merchant
                .record_payment_request(
                    "timada-dashboard",
                    cart_id,
                    pending,
                    timada_orders::OpenedPayment {
                        provider: "stripe".into(),
                        payment_id: payment.payment_id.clone(),
                        client_secret: "secret_for_test".into(),
                        publishable_key: "pk_test".into(),
                    },
                )
                .await
                .unwrap();
            Some(payment)
        }
        Ok(None) => None,
        Err(err) => {
            merchant
                .refuse_payment_request(
                    "timada-dashboard",
                    cart_id,
                    &request_id,
                    err.code().unwrap(),
                )
                .await
                .unwrap();
            None
        }
    }
}

/// What the back office would show for each category, by slug: the same
/// counting the shop does, read from the platform's own read model.
async fn dashboard_counts(h: &Harness, tenant: &TenantState) -> Vec<(String, i64)> {
    // The dashboard's read model is fed by the tenant's own subscription; in
    // this test nothing runs it, so catch it up by hand.
    let catalog = h.catalog(tenant);
    timada_catalog::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(catalog.executor())
        .await
        .unwrap();
    let mut out: Vec<(String, i64)> = categories_of_tenant(&h.db, &tenant.id)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.slug, row.subtree_count))
        .collect();
    out.sort();
    out
}

/// What the shop would show for each category, by slug.
async fn storefront_counts(db: &Db) -> Vec<(String, i64)> {
    fn walk(nodes: &[timada_storefront::CategoryNode], out: &mut Vec<(String, i64)>) {
        for node in nodes {
            out.push((node.category.slug.clone(), node.product_count));
            walk(&node.children, out);
        }
    }
    let mut out = Vec::new();
    walk(&published_categories(db).await.unwrap(), &mut out);
    out.sort();
    out
}

#[tokio::test]
async fn the_back_office_and_the_shop_count_a_category_the_same_way() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme);

    let bedding = catalog
        .create_category(
            "alice",
            CreateCategory {
                name: "Bedding".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mats = catalog
        .create_category(
            "alice",
            CreateCategory {
                name: "Mats".into(),
                slug: None,
                parent_id: Some(bedding.id.clone()),
            },
        )
        .await
        .unwrap();
    // One filed only under the sub-category, one under both.
    stock(&catalog, "Cooling Mat", 2990, &[&mats.id]).await;
    stock(&catalog, "Wool Mat", 1990, &[&bedding.id, &mats.id]).await;

    let db = h.local_db("acme-shop").await;
    let store = Store::connect(h.config(&token), db.clone()).await.unwrap();
    eventually("both products reach the shop", async || {
        published(&db).await.len() == 2
    })
    .await;

    // A product in the sub-category counts for the parent's page, and one in
    // both is counted once rather than twice.
    let expected = vec![("bedding".to_owned(), 2), ("mats".to_owned(), 2)];
    assert_eq!(storefront_counts(&db).await, expected);
    assert_eq!(dashboard_counts(&h, &acme).await, expected);

    // Taking a product out of the parent changes neither page: it is still
    // filed under the sub-category, which the parent's page covers.
    let wool = published_product_by_slug(&db, "wool-mat")
        .await
        .unwrap()
        .unwrap();
    catalog
        .uncategorize_product("alice", &wool.id, &bedding.id)
        .await
        .unwrap();
    eventually("the removal reaches the shop", async || {
        published_product_categories(&db, &wool.id)
            .await
            .unwrap()
            .len()
            == 1
    })
    .await;
    assert_eq!(storefront_counts(&db).await, expected);
    assert_eq!(dashboard_counts(&h, &acme).await, expected);

    // Out of the sub-category as well, and it leaves both pages.
    catalog
        .uncategorize_product("alice", &wool.id, &mats.id)
        .await
        .unwrap();
    eventually("and so does the second removal", async || {
        storefront_counts(&db).await == vec![("bedding".to_owned(), 1), ("mats".to_owned(), 1)]
    })
    .await;
    assert_eq!(
        dashboard_counts(&h, &acme).await,
        vec![("bedding".to_owned(), 1), ("mats".to_owned(), 1)]
    );

    store.shutdown().await;
}

#[tokio::test]
async fn a_store_with_a_checkout_key_sells() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h
        .key_with(&acme, &[SCOPE_STORE_READ, SCOPE_STORE_CHECKOUT])
        .await;
    let (read_token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme);
    let mug = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "Blue Mug".into(),
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

    let api = dashboard_endpoint(h.platform.0.clone()).await;
    let db = h.local_db("selling-shop").await;
    let store = Store::connect(h.config(&token).api_url(&api), db.clone())
        .await
        .unwrap();
    eventually("the product is in the local copy", async || {
        published(&db).await == vec!["Blue Mug".to_owned()]
    })
    .await;

    // The first add makes the cart; the next ones go to it.
    let cart_id = store
        .add_to_cart(None, &mug.id, None, 2)
        .await
        .unwrap()
        .unwrap();
    let same = store
        .add_to_cart(Some(&cart_id), &mug.id, None, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(same, cart_id);
    let refused = store
        .add_to_cart(Some(&cart_id), &mug.id, None, 0)
        .await
        .unwrap();
    assert!(
        matches!(refused, Err(timada_storefront::OrderError::InvalidQuantity)),
        "{refused:?}"
    );

    // Read back at once, priced from the store, illustrated from the copy.
    let cart = store.cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(cart.item_count, 3);
    assert_eq!(cart.total().as_deref(), Some("37.50 €"));
    assert_eq!(cart.lines[0].slug.as_deref(), Some("blue-mug"));
    assert!(cart.can_check_out());

    // The price moves: the total the shopper saw is refused by the
    // dashboard, the new one gets a payment to confirm.
    catalog
        .price_product("alice", &mug.id, 1300, "EUR")
        .await
        .unwrap();
    let checkout = |expected| timada_storefront::Checkout {
        email: "ada@example.org".into(),
        shipping: timada_storefront::ShippingAddress {
            name: "Ada".into(),
            line1: "1 rue de la Paix".into(),
            postal_code: "75002".into(),
            city: "Paris".into(),
            country: "FR".into(),
            ..Default::default()
        },
        note: None,
        expected_total_cents: Some(expected),
        vat_number: None,
        billing: None,
        shipping_method: None,
        language: None,
        newsletter: false,
        time_zone: None,
    };
    let merchant = timada_orders::Command::new(catalog.executor().clone(), catalog.tenant());
    let asking = tokio::spawn({
        let store = store.clone();
        let cart_id = cart_id.clone();
        let checkout = checkout(cart.total_cents);
        async move { store.start_payment(&cart_id, checkout).await }
    });
    answer_as_dashboard(&merchant, &cart_id).await;
    let moved = asking.await.unwrap().unwrap();
    assert!(
        matches!(&moved, Err(err) if err.code() == Some("price_changed")),
        "{moved:?}"
    );
    let asking = tokio::spawn({
        let store = store.clone();
        let cart_id = cart_id.clone();
        let checkout = checkout(3900);
        async move { store.start_payment(&cart_id, checkout).await }
    });
    let payment = answer_as_dashboard(&merchant, &cart_id).await.unwrap();
    let session = asking.await.unwrap().unwrap().unwrap();
    assert_eq!(session.total(), "39.00 €");
    assert_eq!(session.client_secret, "secret_for_test");
    assert_eq!(
        store
            .payment_session(&cart_id, &session.request_id)
            .await
            .unwrap()
            .as_ref(),
        Some(&session)
    );
    assert!(store.order_of_cart(&cart_id).await.unwrap().is_none());

    // The provider confirms to the dashboard: the order exists, paid.
    let placed = merchant
        .place_paid_order("timada-dashboard", &cart_id, payment)
        .await
        .unwrap();
    let order = store.order_of_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(order.reference, placed.reference);
    assert_eq!(order.total_cents, 3900);
    assert_eq!(order.status, timada_orders::OrderStatus::Paid);
    let cart = store.cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(cart.order_id.as_deref(), Some(placed.order_id.as_str()));
    // A cart already ordered: the next add starts a new one.
    let next = store
        .add_to_cart(Some(&cart_id), &mug.id, None, 1)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(next, cart_id);

    // The back office lists it.
    timada_orders::read_model_subscription::<cluster::Executor>(h.db.clone())
        .run_once(catalog.executor())
        .await
        .unwrap();
    let rows = timada_orders::orders_of_tenant(
        &h.db,
        &acme.id,
        &timada_orders::OrderFilter::default(),
        0,
        10,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reference, placed.reference);
    assert_eq!(rows[0].total_label(), "39.00 €");

    // A read-only key shows the catalog but cannot sell: the dashboard
    // refuses it (403), and the store API would its reads.
    let read_db = h.local_db("window-shop").await;
    let window = Store::connect(h.config(&read_token).api_url(&api), read_db)
        .await
        .unwrap();
    let err = window
        .add_to_cart(None, &mug.id, None, 1)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("store:checkout"), "{err:#}");
    let err = window.order_of_cart(&cart_id).await.unwrap_err();
    assert!(format!("{err:#}").contains("store:checkout"), "{err:#}");

    window.shutdown().await;
    store.shutdown().await;
    h.finish().await;
}

#[tokio::test]
async fn a_checkout_knows_how_to_pay_and_the_vat_in_the_total() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h
        .key_with(&acme, &[SCOPE_STORE_READ, SCOPE_STORE_CHECKOUT])
        .await;
    let catalog = h.catalog(&acme);
    let sell = async |name: &str, cents: u64, tax_category: Option<&str>| {
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
            .price_product("alice", &product.id, cents, "EUR")
            .await
            .unwrap();
        catalog
            .set_product_tax_category("alice", &product.id, tax_category)
            .await
            .unwrap();
        catalog.publish_product("alice", &product.id).await.unwrap();
        product.id
    };
    let mug = sell("Blue Mug", 1250, None).await;
    let book = sell("Cat Book", 1000, Some("books")).await;

    let api = dashboard_endpoint(h.platform.0.clone()).await;
    let db = h.local_db("vat-shop").await;
    let store = Store::connect(h.config(&token).api_url(api), db.clone())
        .await
        .unwrap();

    // No payments yet; once the dashboard says how, the store knows.
    assert_eq!(store.payment_setup().await.unwrap(), None);
    let setup = timada_storefront::PaymentSetup {
        provider: "stripe".into(),
        publishable_key: "pk_test_123".into(),
    };
    timada_orders::Command::new(catalog.executor().clone(), catalog.tenant())
        .set_payment_method("timada-dashboard", Some(setup.clone()))
        .await
        .unwrap();
    assert_eq!(store.payment_setup().await.unwrap(), Some(setup));

    // A mug: 20 % French VAT is in its price, and in Germany's too while
    // the merchant sells little abroad; none on an export.
    let cart_id = store
        .add_to_cart(None, &mug, None, 1)
        .await
        .unwrap()
        .unwrap();
    let cart = store.cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(cart.lines[0].tax_category, "standard");
    let fr = cart.vat_estimate("FR").unwrap();
    assert_eq!((fr.vat_cents, fr.amount()), (208, "2.08 €".to_owned()));
    assert_eq!(fr.label(), "VAT FR 20%");
    assert_eq!(cart.vat_estimate("de").unwrap().label(), "VAT FR 20%");
    assert_eq!(cart.vat_estimate("US"), None);

    // With a book at 5.5 %, each rate on its own sum.
    store
        .add_to_cart(Some(&cart_id), &book, None, 1)
        .await
        .unwrap()
        .unwrap();
    let cart = store.cart(&cart_id).await.unwrap().unwrap();
    let line = cart.lines.iter().find(|line| line.product_id == book);
    assert_eq!(line.unwrap().tax_category, "books");
    let fr = cart.vat_estimate("FR").unwrap();
    assert_eq!((fr.rate_bps, fr.vat_cents), (None, 208 + 52));
    assert_eq!(fr.label(), "VAT FR");

    store.shutdown().await;
    h.finish().await;
}

#[tokio::test]
async fn a_store_shows_the_checkout_offers_and_prices_one_unit() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h
        .key_with(&acme, &[SCOPE_STORE_READ, SCOPE_STORE_CHECKOUT])
        .await;
    let catalog = h.catalog(&acme);
    let sell = async |name: &str, cents: u64| {
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
            .price_product("alice", &product.id, cents, "EUR")
            .await
            .unwrap();
        catalog.publish_product("alice", &product.id).await.unwrap();
        product.id
    };
    let mug = sell("Blue Mug", 1250).await;
    let tracker = sell("GPS Tracker", 3000).await;
    let offer_id = timada_discounts::Command::new(catalog.executor().clone(), catalog.tenant())
        .create_offer(
            "alice",
            timada_discounts::CreateOffer {
                product_id: tracker.clone(),
                price_cents: 1200,
                currency: "EUR".into(),
                title: Some("A tracker at 60% off".into()),
                requires_product_id: Some(mug.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let api = dashboard_endpoint(h.platform.0.clone()).await;
    let db = h.local_db("offer-shop").await;
    let store = Store::connect(h.config(&token).api_url(api), db.clone())
        .await
        .unwrap();
    eventually("both products are in the local copy", async || {
        published(&db).await.len() == 2
    })
    .await;

    // A mug in the cart: the tracker is offered, illustrated from the copy.
    let cart_id = store
        .add_to_cart(None, &mug, None, 1)
        .await
        .unwrap()
        .unwrap();
    let cart = store.cart(&cart_id).await.unwrap().unwrap();
    let [offer] = cart.offers.as_slice() else {
        panic!("one offer: {:?}", cart.offers);
    };
    assert_eq!(offer.offer_id, offer_id);
    assert_eq!(offer.slug.as_deref(), Some("gps-tracker"));
    assert_eq!(offer.heading(), "A tracker at 60% off");
    assert_eq!(
        (offer.price(), offer.offer_price(), offer.percent_off()),
        ("30.00 €".to_owned(), "12.00 €".to_owned(), 60)
    );

    // Two trackers added: one line, one unit of it at the offer.
    store
        .add_to_cart(Some(&cart_id), &tracker, None, 2)
        .await
        .unwrap()
        .unwrap();
    let cart = store.cart(&cart_id).await.unwrap().unwrap();
    assert!(cart.offers.is_empty());
    assert_eq!(cart.lines.len(), 2);
    let line = &cart.lines[1];
    assert_eq!(line.quantity, 2);
    assert_eq!(line.unit_amount_cents, Some(3000));
    assert_eq!(line.amount_cents, Some(4200));
    let applied = line.offer.as_ref().unwrap();
    assert_eq!(
        (applied.offer_id.as_str(), applied.unit_amount_cents),
        (offer_id.as_str(), 1200)
    );
    assert_eq!(cart.item_count, 3);
    assert_eq!(cart.total().as_deref(), Some("54.50 €"));
    assert_eq!(cart.vat_estimate("FR").unwrap().vat_cents, 908);

    store.shutdown().await;
    h.finish().await;
}

/// A paid order of one of each product, written as the payments webhook
/// would: what a review is written from.
async fn paid_order(
    executor: &cluster::Executor,
    tenant_id: &str,
    order_id: &str,
    products: &[(&str, &str)],
) {
    let lines = products
        .iter()
        .map(|(id, name)| timada_orders::OrderLine {
            product_id: (*id).to_owned(),
            name: (*name).to_owned(),
            unit_amount_cents: 1000,
            quantity: 1,
            ..Default::default()
        })
        .collect();
    evento::append(order_id)
        .tenant(tenant_id)
        .original_version(0)
        .event(&timada_orders::aggregator::Placed {
            reference: "ABCD-EFGH".into(),
            cart_id: format!("cart-{order_id}"),
            email: "ada@example.org".into(),
            shipping: timada_orders::ShippingAddress {
                name: "Ada Lovelace".into(),
                country: "FR".into(),
                ..Default::default()
            },
            note: None,
            lines,
            currency: "EUR".into(),
            total_cents: 1000 * products.len() as u64,
            payment: timada_orders::Payment {
                provider: "stripe".into(),
                payment_id: format!("pi-{order_id}"),
                amount_cents: 1000 * products.len() as u64,
                currency: "EUR".into(),
            },
            placed_by: "storefront".into(),
        })
        .commit(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_store_shows_the_published_reviews_and_sorts_by_them() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme);
    let kitchen = catalog
        .create_category(
            "alice",
            CreateCategory {
                name: "Kitchen".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mug = stock(&catalog, "Mug", 1000, &[&kitchen.id]).await;
    let plate = stock(&catalog, "Plate", 1000, &[&kitchen.id]).await;
    let bowl = stock(&catalog, "Bowl", 1000, &[&kitchen.id]).await;
    let executor = catalog.executor().clone();
    let reviews = timada_reviews::Command::new(executor.clone(), catalog.tenant());
    let write = |order: &str, product: &str, rating: u8| timada_reviews::SubmitReview {
        order_id: order.to_owned(),
        product_id: product.to_owned(),
        rating,
        title: None,
        body: format!("{rating} stars, honestly."),
        photos: vec![timada_reviews::ReviewPhoto {
            id: format!("photo-{order}-{rating}"),
            ext: "png".into(),
            mime: "image/png".into(),
            size: 68,
            width: Some(1),
            height: Some(1),
        }],
    };
    paid_order(
        &executor,
        catalog.tenant(),
        "order-1",
        &[(&mug, "Mug"), (&plate, "Plate")],
    )
    .await;
    paid_order(
        &executor,
        catalog.tenant(),
        "order-2",
        &[(&mug, "Mug"), (&bowl, "Bowl")],
    )
    .await;
    let mug_5 = reviews
        .submit_review("storefront", write("order-1", &mug, 5))
        .await
        .unwrap();
    let plate_3 = reviews
        .submit_review("storefront", write("order-1", &plate, 3))
        .await
        .unwrap();
    let mug_4 = reviews
        .submit_review("storefront", write("order-2", &mug, 4))
        .await
        .unwrap();
    // Waiting for the merchant: not on the shop.
    reviews
        .submit_review("storefront", write("order-2", &bowl, 5))
        .await
        .unwrap();
    for review in [&mug_5, &plate_3, &mug_4] {
        reviews.publish_review("alice", &review.id).await.unwrap();
    }

    let db = h.local_db("acme-shop").await;
    let store = Store::connect(
        h.config(&token).media_url("https://media.example/acme"),
        db.clone(),
    )
    .await
    .unwrap();
    let rated = async |query: ProductQuery| -> Vec<(String, i64, Option<f64>)> {
        published_products(&db, &query.sort(ProductSort::TopRated))
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|p| (p.name.clone(), p.review_count, p.rating()))
            .collect()
    };
    let expected = vec![
        ("Mug".to_owned(), 2, Some(4.5)),
        ("Plate".to_owned(), 1, Some(3.0)),
        ("Bowl".to_owned(), 0, None),
    ];
    eventually("the published reviews are counted", async || {
        rated(ProductQuery::default()).await == expected
    })
    .await;
    // A category's page sorts the same way, from its own copy.
    assert_eq!(
        rated(ProductQuery::default().in_category(&kitchen.id)).await,
        expected
    );
    let page = store.reviews_of(&mug, 1).await.unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.items[0].author_name, "Ada L.");
    assert_eq!(
        page.items[0].photos[0].url,
        format!(
            "https://media.example/acme/{}",
            page.items[0].photos[0].file
        )
    );
    assert!(store.reviews_of(&bowl, 1).await.unwrap().items.is_empty());
    assert_eq!(page.items[0].reply, None);
    // The merchant's answer follows the review, rewritten or taken off.
    let reply_of = async |id: &str| -> Option<String> {
        store
            .reviews_of(&mug, 1)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|review| review.id == id)
            .and_then(|review| review.reply)
    };
    reviews
        .reply_to_review("alice", &mug_4.id, "Thank you!")
        .await
        .unwrap();
    reviews
        .reply_to_review("alice", &mug_4.id, "Thank you, Ada!")
        .await
        .unwrap();
    eventually("the reply is copied", async || {
        reply_of(&mug_4.id).await.as_deref() == Some("Thank you, Ada!")
    })
    .await;
    reviews
        .remove_review_reply("alice", &mug_4.id)
        .await
        .unwrap();
    eventually("the reply leaves", async || {
        reply_of(&mug_4.id).await.is_none()
    })
    .await;
    // The shopper's own review is read from the store, waiting or not.
    let own = store.review_of("order-2", &bowl).await.unwrap().unwrap();
    assert_eq!(own.status, timada_storefront::ReviewStatus::Pending);
    assert!(store.review_of("order-1", &bowl).await.unwrap().is_none());

    // Taken down, the mug's best review leaves and its average falls.
    reviews.reject_review("alice", &mug_5.id, "").await.unwrap();
    eventually("the rejected review leaves", async || {
        rated(ProductQuery::default()).await
            == vec![
                ("Mug".to_owned(), 1, Some(4.0)),
                ("Plate".to_owned(), 1, Some(3.0)),
                ("Bowl".to_owned(), 0, None),
            ]
    })
    .await;
    assert_eq!(store.reviews_of(&mug, 1).await.unwrap().total, 1);
    // Without the dashboard's address no review can be handed in.
    assert!(!store.takes_reviews());
    assert!(
        store
            .submit_review("order-1", &mug, Default::default())
            .await
            .is_err()
    );

    store.shutdown().await;
    h.finish().await;
}

/// The dashboard's `POST /store/shoppers/code` as far as its limits go: the
/// key is checked against the platform, then the request is held to the
/// same limits with the same inputs, and answered 202 or 429 as the
/// dashboard does. No mail goes out. Returns the address to give the store.
/// One HTTP request off `socket`: its head (the request line and headers)
/// and its body, read to `content-length`.
async fn read_request(socket: &mut TcpStream) -> (String, Vec<u8>) {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut buf).await.unwrap();
        assert!(n > 0, "the request ended before its headers did");
        request.extend_from_slice(&buf[..n]);
        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
    let length: usize = header(&head, "content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while request.len() < head_end + length {
        let n = socket.read(&mut buf).await.unwrap();
        assert!(n > 0, "the request ended before its body did");
        request.extend_from_slice(&buf[..n]);
    }
    let body = request[head_end..].to_vec();
    (head, body)
}

fn header(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name)
            .then(|| value.trim().to_owned())
    })
}

/// Answers `status` with a JSON body and closes the connection.
async fn respond(mut socket: TcpStream, status: StatusCode, body: &Value) {
    use tokio::io::AsyncWriteExt;

    let body = body.to_string();
    let answer = format!(
        "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        body.len()
    );
    socket.write_all(answer.as_bytes()).await.unwrap();
    socket.shutdown().await.ok();
}

/// The dashboard's `POST /store/shoppers/code`, as far as the limits go.
async fn code_endpoint(platform: cluster::Executor, limits: config::StoreConfig) -> String {
    let limiter = Arc::new(rate_limit::RateLimiter::new(None));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, peer)) = listener.accept().await else {
                return;
            };
            let (platform, limits, limiter) = (platform.clone(), limits.clone(), limiter.clone());
            tokio::spawn(async move {
                let (head, _body) = read_request(&mut socket).await;
                assert!(head.starts_with("POST /store/shoppers/code "), "{head}");
                let token = header(&head, "authorization").unwrap();
                let access = Command(platform)
                    .authenticate_api_key(token.strip_prefix("Bearer ").unwrap())
                    .await
                    .unwrap();
                let allowed = rate_limit::allow_shopper_code(
                    &limiter,
                    &limits,
                    &access.key.id,
                    &access.tenant.id,
                    header(&head, "x-shopper-ip").as_deref(),
                    Some(peer.ip()),
                )
                .await;
                let (status, body) = if allowed {
                    (StatusCode::ACCEPTED, json!({}))
                } else {
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        json!({ "error": "too_many_requests" }),
                    )
                };
                respond(socket, status, &body).await;
            });
        }
    });
    url
}

/// The dashboard's `/store/carts/*` and `/store/orders/*` side, as the
/// library sees it: the key authenticated (401 unknown, 403 without
/// `store:checkout`) and the path handed to `store_carts` in the key's
/// tenant, over a socket of its own.
async fn dashboard_endpoint(platform: cluster::Executor) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let platform = platform.clone();
            tokio::spawn(async move {
                let (head, body) = read_request(&mut socket).await;
                let mut request_line = head.lines().next().unwrap_or_default().split(' ');
                let method = request_line.next().unwrap_or_default().to_owned();
                let path = request_line.next().unwrap_or_default().to_owned();
                let token = header(&head, "authorization")
                    .and_then(|value| value.strip_prefix("Bearer ").map(str::to_owned));
                let access = match token {
                    Some(token) => Command(platform.clone()).authenticate_api_key(&token).await,
                    None => Err(timada_tenancy::TenancyError::ApiKeyInvalid),
                };
                let (status, answer) = match access {
                    Err(_) => (StatusCode::UNAUTHORIZED, json!({})),
                    Ok(access) if !access.key.allows(SCOPE_STORE_CHECKOUT) => {
                        (StatusCode::FORBIDDEN, json!({}))
                    }
                    Ok(access) => {
                        let orders = store_carts::gateway(platform, &access.tenant.id);
                        dispatch(&orders, &method, &path, &body).await.unwrap()
                    }
                };
                respond(socket, status, &answer).await;
            });
        }
    });
    url
}

/// The routes under `/store/carts` and `/store/orders`, as
/// `dashboard/src/app/store` lays them out.
async fn dispatch(
    orders: &timada_orders::Command<cluster::Executor>,
    method: &str,
    path: &str,
    body: &[u8],
) -> anyhow::Result<store_carts::Answer> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    Ok(match (method, segments.as_slice()) {
        ("POST", ["store", "carts"]) => match store_carts::parse(body) {
            Some(item) => store_carts::start(orders, item).await?,
            None => store_carts::bad_request(),
        },
        ("GET", ["store", "carts", id]) => store_carts::cart(orders, id).await?,
        ("POST", ["store", "carts", id, "items"]) => match store_carts::parse(body) {
            Some(item) => store_carts::add_item(orders, id, item).await?,
            None => store_carts::bad_request(),
        },
        ("POST", ["store", "carts", id, "quantity"]) => match store_carts::parse(body) {
            Some(item) => store_carts::set_quantity(orders, id, item).await?,
            None => store_carts::bad_request(),
        },
        ("POST", ["store", "carts", id, "discount"]) => {
            match store_carts::parse::<timada_orders::wire::DiscountCode>(body) {
                Some(code) => store_carts::apply_discount(orders, id, &code.code).await?,
                None => store_carts::bad_request(),
            }
        }
        ("POST", ["store", "carts", id, "discount", "remove"]) => {
            store_carts::remove_discount(orders, id).await?
        }
        ("POST", ["store", "carts", id, "checkout"]) => match store_carts::parse(body) {
            Some(checkout) => store_carts::checkout(orders, id, checkout).await?,
            None => store_carts::bad_request(),
        },
        ("GET", ["store", "carts", id, "requests", request_id]) => {
            store_carts::request(orders, id, request_id).await?
        }
        ("GET", ["store", "carts", id, "order"]) => store_carts::order_of_cart(orders, id).await?,
        ("GET", ["store", "orders", id]) => store_carts::order(orders, id).await?,
        ("GET", ["store", "orders", id, "documents"]) => store_carts::documents(orders, id).await?,
        _ => (StatusCode::NOT_FOUND, json!({ "error": "no_such_route" })),
    })
}

#[tokio::test]
async fn the_dashboard_answers_for_one_cart_or_order_of_the_key_tenant() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let globex = h.create("globex").await;
    let catalog = h.catalog(&acme);
    let mug = catalog
        .create_product(
            "alice",
            CreateProduct {
                name: "Blue Mug".into(),
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
    let acme_orders = store_carts::gateway(h.platform.0.clone(), &acme.id);
    let globex_orders = store_carts::gateway(h.platform.0.clone(), &globex.id);
    let item = timada_orders::CartItem {
        product_id: mug.id.clone(),
        variant_id: None,
        quantity: 1,
    };

    // Acme's storefront starts a cart; Globex's, holding the same id, is
    // told there is no such cart, as for any id the tenant has not.
    let (status, started) = store_carts::start(&acme_orders, item.clone())
        .await
        .unwrap();
    assert_eq!(status, StatusCode::CREATED);
    let cart_id = started["id"].as_str().unwrap().to_owned();
    let (status, answer) = store_carts::cart(&acme_orders, &cart_id).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["cart"]["id"], json!(cart_id));
    assert_eq!(answer["quote"]["total_cents"], json!(1250));
    for (status, answer) in [
        store_carts::cart(&globex_orders, &cart_id).await.unwrap(),
        store_carts::cart(&acme_orders, "01J00000000000000000000000")
            .await
            .unwrap(),
        store_carts::add_item(&globex_orders, &cart_id, item.clone())
            .await
            .unwrap(),
        store_carts::request(&globex_orders, &cart_id, "req")
            .await
            .unwrap(),
    ] {
        assert_eq!(
            (status, &answer),
            (StatusCode::NOT_FOUND, &json!({ "error": "cart_not_found" }))
        );
    }
    // The request to pay is Acme's cart's too: its request is not Globex's.
    let (status, _) = store_carts::request(&acme_orders, &cart_id, "req")
        .await
        .unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A refusal travels with its code.
    let (status, answer) = store_carts::add_item(
        &acme_orders,
        &cart_id,
        timada_orders::CartItem {
            quantity: 0,
            ..item.clone()
        },
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(answer, json!({ "error": "invalid_quantity" }));

    // An order Acme placed: Acme's storefront reads it by its id or its
    // cart's, Globex's is told there is none.
    paid_order(&h.platform.0, &acme.id, "order-1", &[(&mug.id, "Blue Mug")]).await;
    let (status, answer) = store_carts::order(&acme_orders, "order-1").await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["reference"], json!("ABCD-EFGH"));
    for (status, answer) in [
        store_carts::order(&globex_orders, "order-1").await.unwrap(),
        store_carts::order(&acme_orders, "order-2").await.unwrap(),
        store_carts::order_of_cart(&acme_orders, &cart_id)
            .await
            .unwrap(),
        store_carts::order_of_cart(&globex_orders, "cart-order-1")
            .await
            .unwrap(),
    ] {
        assert_eq!(
            (status, &answer),
            (
                StatusCode::NOT_FOUND,
                &json!({ "error": "order_not_found" })
            )
        );
    }
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_codes_are_held_to_a_limit_per_shopper_and_per_key() {
    let h = harness(None).await;
    let tenant = h.create("acme").await;
    let (token, _) = h
        .key_with(&tenant, &[SCOPE_STORE_READ, SCOPE_STORE_CHECKOUT])
        .await;
    let api = code_endpoint(
        h.platform.0.clone(),
        config::StoreConfig {
            shopper_codes_per_ip: 2,
            shopper_codes_per_key: 4,
            shopper_codes_window_secs: 600,
            ..config::StoreConfig::default()
        },
    )
    .await;
    let store = Store::connect(h.config(&token).api_url(api), h.local_db("codes").await)
        .await
        .unwrap();
    let ask = async |ip: Option<&str>| {
        store
            .request_sign_in_code("ada@example.com", ip.map(|ip| ip.parse().unwrap()), None)
            .await
            .unwrap()
            .map_err(|refusal| refusal.code())
    };

    // One shopper gets their two codes, the third is refused, and the
    // shopper next to them still gets theirs.
    assert_eq!(ask(Some("203.0.113.7")).await, Ok(()));
    assert_eq!(ask(Some("203.0.113.7")).await, Ok(()));
    assert_eq!(ask(Some("203.0.113.7")).await, Err("too_many_requests"));
    assert_eq!(ask(Some("2001:db8::7")).await, Ok(()));
    // Without the shopper's address, the storefront's own counts.
    assert_eq!(ask(None).await, Ok(()));
    // The key has had four codes mailed (the refusal cost it none): no
    // more, whoever the shopper.
    assert_eq!(ask(Some("198.51.100.9")).await, Err("too_many_requests"));

    store.shutdown().await;
    h.finish().await;
}

#[tokio::test]
async fn a_product_leaves_a_category_it_was_filed_under() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let catalog = h.catalog(&acme);
    let bedding = catalog
        .create_category(
            "alice",
            timada_catalog::CreateCategory {
                name: "Bedding".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
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
        .categorize_product("alice", &mug.id, &bedding.id)
        .await
        .unwrap();
    catalog
        .uncategorize_product("alice", &mug.id, &bedding.id)
        .await
        .unwrap();
    h.finish().await;
}

#[tokio::test]
async fn a_store_shows_the_published_pages() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let pages = h.pages(&acme);
    let page = |title: &str, body: &str| timada_pages::CreatePage {
        slug: String::new(),
        title: title.into(),
        body: body.into(),
    };
    let terms = pages
        .create_page(
            "alice",
            page("Conditions de vente", "## Prix\n\nEn **euros**."),
        )
        .await
        .unwrap();
    let legal = pages
        .create_page("alice", page("Mentions légales", "Édité par Acme."))
        .await
        .unwrap();
    pages
        .create_page("alice", page("Brouillon", "Pas encore."))
        .await
        .unwrap();
    let image = timada_pages::new_page_image_id();
    pages
        .add_page_image(
            "alice",
            &legal,
            timada_pages::AddPageImage {
                image_id: image.clone(),
                ext: "png".into(),
                size: 10,
                width: None,
                height: None,
            },
        )
        .await
        .unwrap();
    pages
        .edit_page(
            "alice",
            &legal,
            "Mentions légales",
            &format!("Édité par Acme.\n\n![Le siège]({image}.png)"),
        )
        .await
        .unwrap();
    pages.publish_page("alice", &terms).await.unwrap();
    pages.publish_page("alice", &legal).await.unwrap();
    pages
        .set_page_seo("alice", &terms, "CGV | Acme", "")
        .await
        .unwrap();

    let db = h.local_db("acme-shop").await;
    let config = h.config(&token).media_url("https://cdn.example/media/acme");
    let store = Store::connect(config, db.clone()).await.unwrap();
    eventually("the published pages reach the local copy", async || {
        published_page(&db, "mentions-legales", None)
            .await
            .unwrap()
            .is_some()
    })
    .await;

    // The draft has no page.
    assert!(
        published_page(&db, "brouillon", None)
            .await
            .unwrap()
            .is_none()
    );

    let shown = published_page(&db, "conditions-de-vente", store.media_url())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shown.title, "Conditions de vente");
    assert!(shown.html.contains("<h2>Prix</h2>"), "{}", shown.html);
    assert!(
        shown.html.contains("<strong>euros</strong>"),
        "{}",
        shown.html
    );
    assert_eq!(shown.meta_title, "CGV | Acme");
    assert_eq!(shown.meta_description, "Prix En euros.");
    let legal_page = published_page(&db, "mentions-legales", store.media_url())
        .await
        .unwrap()
        .unwrap();
    assert!(
        legal_page.html.contains(&format!(
            r#"src="https://cdn.example/media/acme/{image}.png""#
        )),
        "{}",
        legal_page.html
    );

    // Search matches the start of a word, title matches first.
    let found = search_published_pages(&db, "edit", 5).await.unwrap();
    assert_eq!(
        found
            .iter()
            .map(|link| link.slug.as_str())
            .collect::<Vec<_>>(),
        ["mentions-legales"]
    );

    // Unpublished, moved and deleted pages follow.
    pages.unpublish_page("alice", &terms).await.unwrap();
    pages.rename_page("alice", &legal, "legal").await.unwrap();
    eventually("a moved page is found at its new slug", async || {
        published_page(&db, "legal", None).await.unwrap().is_some()
    })
    .await;
    assert!(
        published_page(&db, "mentions-legales", None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        published_page(&db, "conditions-de-vente", None)
            .await
            .unwrap()
            .is_none()
    );
    pages.delete_page("alice", &legal).await.unwrap();
    eventually("a deleted page leaves the copy", async || {
        published_page(&db, "legal", None).await.unwrap().is_none()
    })
    .await;

    store.shutdown().await;
}
