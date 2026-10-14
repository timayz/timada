//! The headless storefront library against the store API on one node: a
//! `Store` follows its tenant's catalog into a local database, keeps its
//! cursor there and nowhere else, resumes from it after a restart, is cut
//! off with its key, connects over TLS, and carries the category tree the
//! products are filed under.
//!
//! The dashboard is a binary crate, so the modules are reached through the
//! binary's own test harness (`#[path]` includes), which drags in items this
//! test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/read_only_store.rs"]
mod read_only_store;
#[path = "../src/store_api.rs"]
mod store_api;
#[path = "../src/tenants.rs"]
mod tenants;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use evento::migrator::Migrate;
use evento::remote::ServerHandle;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_catalog::{
    AddImage, CreateCategory, CreateProduct, DefineVariant, OptionSelection, categories_of_tenant,
};
use timada_storefront::{
    CATALOG_SUBSCRIPTION, ProductQuery, ProductSort, Store, StoreConfig, StoreTls,
    SubscriptionState, category_ancestors, find_category, published_categories,
    published_category_by_slug, published_product_by_slug, published_product_categories,
    published_products, published_related_products,
};
use timada_tenancy::{
    Command, CreateApiKey, CreateTenant, SCOPE_STORE_READ, TenantState, read_model_subscription,
};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use cluster::Cluster;
use config::{StoreApiConfig, TenantsConfig};
use store_api::StoreConnections;
use tenants::{TenantStatus, Tenants};

const NOW: u64 = 1_800_000_000;

struct Harness {
    tenants: Arc<Tenants>,
    platform: Command<cluster::Executor>,
    pool: SqlitePool,
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
            .add_migrations(timada_catalog::migrations())
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
    let tenants = Tenants::new(
        cluster,
        &TenantsConfig {
            dir: dir.join("tenants"),
            idle_timeout_secs: 1,
            max_open: 4,
        },
        None,
        platform.clone(),
        pool.clone(),
        vec![],
    );
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
    let server = store_api::serve(
        Some(&config),
        platform.clone(),
        tenants.clone(),
        connections.clone(),
    )
    .await
    .unwrap()
    .unwrap();
    Harness {
        tenants,
        platform: Command(platform),
        pool,
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
        read_model_subscription::<cluster::Executor>(self.pool.clone())
            .run_once(&self.platform.0)
            .await
            .unwrap();
        tenant
    }

    async fn key(&self, tenant: &TenantState) -> (String, String) {
        let created = self
            .platform
            .create_api_key(
                "alice",
                CreateApiKey {
                    tenant_id: tenant.id.clone(),
                    label: "Shop".into(),
                    scopes: vec![SCOPE_STORE_READ.into()],
                },
                NOW,
            )
            .await
            .unwrap();
        (created.token, created.key.id)
    }

    /// A local database for one storefront replica, as a file: `:memory:`
    /// would give every pooled connection its own empty database.
    async fn local_pool(&self, name: &str) -> SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(self.dir.join(format!("{name}.db")))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        SqlitePoolOptions::new()
            .max_connections(3)
            .connect_with(options)
            .await
            .unwrap()
    }

    fn config(&self, token: &str) -> StoreConfig {
        StoreConfig::new(self.addr, token).request_timeout(Duration::from_secs(2))
    }

    /// The catalog gateway over the tenant's own store, as the dashboard
    /// pages use it.
    async fn catalog(&self, tenant: &TenantState) -> timada_catalog::Command<cluster::Executor> {
        timada_catalog::Command(self.tenants.open(tenant).await.unwrap())
    }

    /// The subscription cursors recorded in a tenant's own file.
    async fn tenant_cursors(&self, tenant: &TenantState) -> Vec<String> {
        let options = SqliteConnectOptions::new()
            .filename(self.tenants.path_of(&tenant.id))
            .read_only(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let keys = sqlx::query_scalar("SELECT key FROM subscriber ORDER BY key")
            .fetch_all(&pool)
            .await
            .unwrap();
        pool.close().await;
        keys
    }

    fn leases(&self, tenant: &TenantState) -> Option<usize> {
        match self.tenants.status(&tenant.id) {
            TenantStatus::Running { leases, .. } => Some(leases),
            TenantStatus::Stopped => None,
        }
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
        self.tenants.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn local_cursors(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar("SELECT key FROM subscriber ORDER BY key")
        .fetch_all(pool)
        .await
        .unwrap()
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

async fn published(pool: &SqlitePool) -> Vec<String> {
    published_products(pool, &ProductQuery::default())
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
    let catalog = h.catalog(&acme).await;
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
        .await
        .create_product(
            "bob",
            CreateProduct {
                name: "Zeta Thing".into(),
                slug: None,
            },
        )
        .await
        .unwrap();

    let pool = h.local_pool("acme-shop").await;
    let store = Store::connect(h.config(&token), pool.clone())
        .await
        .unwrap();
    eventually("the published product is in the local copy", async || {
        published(&pool).await == vec!["Blue Mug".to_owned()]
    })
    .await;
    let found = published_product_by_slug(&pool, "blue-mug")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.price().as_deref(), Some("12.50 EUR"));
    assert!(
        published_product_by_slug(&pool, "draft-plate")
            .await
            .unwrap()
            .is_none(),
        "a draft is not for sale"
    );
    assert!(
        published_product_by_slug(&pool, "zeta-thing")
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
        published(&pool).await == vec!["Big Blue Mug".to_owned()]
    })
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // The cursor is local; the tenant's own file has no cursor of ours.
    assert_eq!(
        local_cursors(&pool).await,
        vec![CATALOG_SUBSCRIPTION.to_owned()]
    );
    assert!(
        h.tenant_cursors(&acme).await.is_empty(),
        "{:?}",
        h.tenant_cursors(&acme).await
    );
    let status = store.status().await;
    assert!(status.connected);
    assert_eq!(status.subscription, SubscriptionState::Running);
    assert_eq!(status.products, 1);
    assert!(status.last_event_at.is_some());
    assert_eq!(h.leases(&acme), Some(1), "the connection leases the tenant");

    store.shutdown().await;
    h.wait_for("the lease is released", |h| h.leases(&acme) == Some(0))
        .await;
    h.finish().await;
}

#[tokio::test]
async fn a_restarted_store_resumes_from_its_local_cursor() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme).await;
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

    let pool = h.local_pool("replica").await;
    let store = Store::connect(h.config(&token), pool.clone())
        .await
        .unwrap();
    eventually("the first product arrives", async || {
        published(&pool).await.len() == 1
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

    let store = Store::connect(h.config(&token), pool.clone())
        .await
        .unwrap();
    eventually("the second product arrives after the restart", async || {
        published(&pool).await.len() == 2
    })
    .await;
    assert_eq!(local_cursors(&pool).await.len(), 1, "one cursor, resumed");
    assert!(store.status().await.last_event_at >= before);

    store.shutdown().await;
    h.finish().await;
}

#[tokio::test]
async fn a_revoked_key_disconnects_the_store() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, key_id) = h.key(&acme).await;
    let pool = h.local_pool("revoked").await;
    let store = Store::connect(h.config(&token), pool.clone())
        .await
        .unwrap();
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
    let err = match Store::connect(h.config(&token), h.local_pool("again").await).await {
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
    let catalog = h.catalog(&acme).await;
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

    let plain = Store::connect(h.config(&token), h.local_pool("plain").await).await;
    assert!(
        plain.is_err(),
        "plain TCP against a TLS listener is refused"
    );

    let pool = h.local_pool("tls").await;
    let tls = StoreTls::from_ca_pem(&cert, "localhost").unwrap();
    let store = Store::connect(h.config(&token).tls(tls), pool.clone())
        .await
        .unwrap();
    eventually("the product arrives over TLS", async || {
        published(&pool).await == vec!["Secure Mug".to_owned()]
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
    let catalog = h.catalog(&acme).await;

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

    let pool = h.local_pool("acme-shop").await;
    let store = Store::connect(h.config(&token), pool.clone())
        .await
        .unwrap();
    eventually(
        "every published product reaches the local copy",
        async || published(&pool).await.len() == 3,
    )
    .await;
    eventually("and so does the category tree", async || {
        published_categories(&pool).await.unwrap().len() == 2
    })
    .await;

    // The tree keeps its order and counts a subtree, not just one level.
    let tree = published_categories(&pool).await.unwrap();
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
    let mats_row = published_category_by_slug(&pool, "mats")
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

    // A collection page asks for a subtree, in the order a shopper picked.
    let cheapest_first = published_products(
        &pool,
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
        &pool,
        &ProductQuery::default()
            .in_category(bedding.id.clone())
            .sort(ProductSort::PriceDesc),
    )
    .await
    .unwrap();
    assert_eq!(dearest_first.items[0].name, "Cooling Mat");
    let searched = published_products(
        &pool,
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
            &pool,
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
    let cooling_row = published_product_by_slug(&pool, "cooling-mat")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        published_related_products(&pool, &cooling_row, 4)
            .await
            .unwrap()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Wool Mat"]
    );
    assert_eq!(
        published_product_categories(&pool, &cooling_row.id)
            .await
            .unwrap()
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Mats"]
    );

    // Leaving a category and archiving one both reach the local copy.
    catalog
        .uncategorize_product("alice", &cooling, &mats.id)
        .await
        .unwrap();
    eventually("a product that left is no longer counted", async || {
        find_category(&published_categories(&pool).await.unwrap(), "mats").map(|n| n.product_count)
            == Some(1)
    })
    .await;
    catalog.archive_category("alice", &toys.id).await.unwrap();
    eventually("an archived category leaves the tree", async || {
        published_categories(&pool).await.unwrap().len() == 1
    })
    .await;
    assert!(
        published_category_by_slug(&pool, "toys")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        published(&pool).await.len(),
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
    let catalog = h.catalog(&acme).await;
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
    let pool = h.local_pool("acme-shop").await;
    let config = h
        .config(&token)
        .media_url("https://cdn.example/media/acme/");
    let store = Store::connect(config, pool.clone()).await.unwrap();
    eventually("the product reaches the local copy", async || {
        published_product_by_slug(&pool, "cooling-mat")
            .await
            .unwrap()
            .is_some_and(|p| p.images.len() == 2)
    })
    .await;

    let mut product = published_product_by_slug(&pool, "cooling-mat")
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
        published_product_by_slug(&pool, "cooling-mat")
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
        published_product_by_slug(&pool, "cooling-mat")
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
    let catalog = h.catalog(&acme).await;

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

    let pool = h.local_pool("acme-shop").await;
    let store = Store::connect(h.config(&token), pool.clone())
        .await
        .unwrap();
    eventually("the varying product reaches the local copy", async || {
        published_product_by_slug(&pool, "cat-harness")
            .await
            .unwrap()
            .is_some_and(|p| p.variants.len() == 3)
    })
    .await;
    eventually("and so does the plain one beside it", async || {
        published(&pool).await.len() == 2
    })
    .await;

    let harness = published_product_by_slug(&pool, "cat-harness")
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
    assert_eq!(harness.price().as_deref(), Some("22.90–24.90 EUR"));
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
    assert_eq!(variant.price(), "24.90 EUR");
    assert_eq!(variant.sku.as_deref(), Some("harness-beige-m"));
    assert_eq!(variant.value_of(&colour), Some("Beige"));
    assert!(
        harness
            .variant(&[(colour.clone(), "Blue".into()), (size.clone(), "M".into())])
            .is_none(),
        "the one combination they did not price is not sold"
    );

    // Sorting goes by what a shopper would pay, not by the base price.
    let cheapest = published_products(&pool, &ProductQuery::default().sort(ProductSort::PriceAsc))
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
    let dearest = published_products(&pool, &ProductQuery::default().sort(ProductSort::PriceDesc))
        .await
        .unwrap();
    assert_eq!(dearest.items[0].name, "Cat Harness");

    // A variant that goes narrows the range.
    catalog
        .remove_variant("alice", &harness_id, &harness.variants[1].id)
        .await
        .unwrap();
    eventually("the narrower range reaches the local copy", async || {
        published_product_by_slug(&pool, "cat-harness")
            .await
            .unwrap()
            .and_then(|p| p.price())
            == Some("22.90 EUR".to_owned())
    })
    .await;

    store.shutdown().await;
}

/// What the back office would show for each category, by slug: the same
/// counting the shop does, read from the platform's own read model.
async fn dashboard_counts(h: &Harness, tenant: &TenantState) -> Vec<(String, i64)> {
    // The dashboard's read model is fed by the tenant's own subscription; in
    // this test nothing runs it, so catch it up by hand.
    let catalog = h.catalog(tenant).await;
    timada_catalog::read_model_subscription::<cluster::Executor>(h.pool.clone(), &tenant.id)
        .run_once(&catalog.0)
        .await
        .unwrap();
    let mut out: Vec<(String, i64)> = categories_of_tenant(&h.pool, &tenant.id)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.slug, row.subtree_count))
        .collect();
    out.sort();
    out
}

/// What the shop would show for each category, by slug.
async fn storefront_counts(pool: &SqlitePool) -> Vec<(String, i64)> {
    fn walk(nodes: &[timada_storefront::CategoryNode], out: &mut Vec<(String, i64)>) {
        for node in nodes {
            out.push((node.category.slug.clone(), node.product_count));
            walk(&node.children, out);
        }
    }
    let mut out = Vec::new();
    walk(&published_categories(pool).await.unwrap(), &mut out);
    out.sort();
    out
}

#[tokio::test]
async fn the_back_office_and_the_shop_count_a_category_the_same_way() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;
    let catalog = h.catalog(&acme).await;

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

    let pool = h.local_pool("acme-shop").await;
    let store = Store::connect(h.config(&token), pool.clone())
        .await
        .unwrap();
    eventually("both products reach the shop", async || {
        published(&pool).await.len() == 2
    })
    .await;

    // A product in the sub-category counts for the parent's page, and one in
    // both is counted once rather than twice.
    let expected = vec![("bedding".to_owned(), 2), ("mats".to_owned(), 2)];
    assert_eq!(storefront_counts(&pool).await, expected);
    assert_eq!(dashboard_counts(&h, &acme).await, expected);

    // Taking a product out of the parent changes neither page: it is still
    // filed under the sub-category, which the parent's page covers.
    let wool = published_product_by_slug(&pool, "wool-mat")
        .await
        .unwrap()
        .unwrap();
    catalog
        .uncategorize_product("alice", &wool.id, &bedding.id)
        .await
        .unwrap();
    eventually("the removal reaches the shop", async || {
        published_product_categories(&pool, &wool.id)
            .await
            .unwrap()
            .len()
            == 1
    })
    .await;
    assert_eq!(storefront_counts(&pool).await, expected);
    assert_eq!(dashboard_counts(&h, &acme).await, expected);

    // Out of the sub-category as well, and it leaves both pages.
    catalog
        .uncategorize_product("alice", &wool.id, &mats.id)
        .await
        .unwrap();
    eventually("and so does the second removal", async || {
        storefront_counts(&pool).await == vec![("bedding".to_owned(), 1), ("mats".to_owned(), 1)]
    })
    .await;
    assert_eq!(
        dashboard_counts(&h, &acme).await,
        vec![("bedding".to_owned(), 1), ("mats".to_owned(), 1)]
    );

    store.shutdown().await;
}
