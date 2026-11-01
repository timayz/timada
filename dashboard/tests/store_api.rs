//! The store API end to end on one node: a storefront connects with a key
//! and is served its tenant's store only; the connection leases the tenant;
//! a revoked key is cut off; a suspended tenant and bad keys are refused; a
//! key without `store:write` reads what a storefront loads and never a cart
//! or an order; the same over TLS.
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
#[path = "../src/read_only_store.rs"]
mod read_only_store;
#[path = "../src/store_api.rs"]
mod store_api;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use evento::Executor as _;
use evento::cursor::{Args, Value};
use evento::remote::{Client, ServerHandle};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use timada_tenancy::aggregator::Renamed;
use timada_tenancy::{
    Command, CreateApiKey, CreateTenant, SCOPE_STORE_CHECKOUT, SCOPE_STORE_READ, SCOPE_STORE_WRITE,
    TenantState, read_model_subscription,
};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use cluster::Cluster;
use config::StoreApiConfig;
use store_api::StoreConnections;

const NOW: u64 = 1_800_000_000;

struct Harness {
    cluster: Arc<Cluster>,
    platform: Command<cluster::Executor>,
    /// The read models in the store's database.
    db: db::Db,
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
    // `RUST_LOG=evento_remote=debug,store_api=debug` shows the connection
    // lifecycle when a run fails; silent otherwise. Tests share one process,
    // so only the first harness installs it.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let dir = std::env::temp_dir().join(format!("timada-store-api-{}", ulid::Ulid::generate()));
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
    // The store in memory (a file would fsync every commit of the thirty-odd
    // tests running at once), the read models in the test's own pool.
    let files = cluster::Files {
        path: PathBuf::from(":memory:"),
        read_models_url: format!("sqlite:{}", dir.join("platform.db").display()),
    };
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    db::migrate_store_read_models(&store).await.unwrap();
    let store_db = db::Db::from(store.clone());
    let (cluster, platform) = Cluster::open(None, None, &files, store).await.unwrap();
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
        db: store_db,
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
        read_model_subscription::<cluster::Executor>(self.db.clone())
            .run_once(&self.platform.0)
            .await
            .unwrap();
        tenant
    }

    async fn key(&self, tenant: &TenantState) -> (String, String) {
        self.key_with(tenant, SCOPE_STORE_WRITE).await
    }

    async fn key_with(&self, tenant: &TenantState, scope: &str) -> (String, String) {
        self.key_with_scopes(tenant, &[scope]).await
    }

    async fn key_with_scopes(&self, tenant: &TenantState, scopes: &[&str]) -> (String, String) {
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

    async fn connect(&self, token: &str) -> anyhow::Result<Client> {
        self.connect_with(Some(token)).await
    }

    async fn connect_with(&self, token: Option<&str>) -> anyhow::Result<Client> {
        // The client's default timeout: the hello opens the tenant (a new
        // SQLite file and its migrations), which a loaded machine can take
        // seconds over.
        let mut builder = Client::builder(self.addr);
        if let Some(token) = token {
            builder = builder.token(token);
        }
        builder.connect().await
    }

    /// The error a connection with `token` fails with (`None`: no token).
    async fn refused(&self, token: Option<&str>) -> String {
        match self.connect_with(token).await {
            Ok(_) => panic!("the connection must be refused"),
            Err(err) => format!("{err:#}"),
        }
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

/// Writes a probe event through `client` (the tenancy events double as a
/// payload) and returns how many events the store holds.
async fn probe(client: &Client, stream: &str, version: u16) -> usize {
    evento::append(stream)
        .original_version(version)
        .event(&Renamed {
            name: "probe".into(),
        })
        .commit(client)
        .await
        .unwrap();
    count(client).await.unwrap()
}

/// Writes a probe event in `tenant`'s streams through the dashboard's own
/// store, as a page would.
async fn probe_as(h: &Harness, tenant: &TenantState, stream: &str, version: u16) {
    evento::append(stream)
        .tenant(&tenant.id)
        .original_version(version)
        .event(&Renamed {
            name: "probe".into(),
        })
        .commit(&h.platform.0)
        .await
        .unwrap();
}

async fn count(client: &Client) -> anyhow::Result<usize> {
    Ok(client
        .read(
            None,
            None,
            evento::TenantFilter::All,
            Args::forward(100, None),
            None,
        )
        .await?
        .edges
        .len())
}

/// Reads the events of one aggregate type through `executor`.
async fn read_type<E: evento::Executor>(
    executor: &E,
    aggregate_type: &str,
) -> anyhow::Result<usize> {
    Ok(executor
        .read(
            Some(Arc::from(vec![evento::EventFilter {
                aggregate_type: aggregate_type.to_owned(),
                aggregate_id: None,
                name: None,
            }])),
            None,
            evento::TenantFilter::All,
            Args::forward(100, None),
            None,
        )
        .await?
        .edges
        .len())
}

#[tokio::test]
async fn a_store_connects_with_its_key_and_leases_its_tenant() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let zeta = h.create("zeta").await;
    let (acme_token, _) = h.key(&acme).await;
    let (zeta_token, _) = h.key(&zeta).await;
    assert_eq!(
        h.connected(&acme),
        0,
        "nothing connected before a connection"
    );

    let client = h.connect(&acme_token).await.unwrap();
    assert_eq!(h.connected(&acme), 1);
    assert_eq!(probe(&client, "s1", 0).await, 1);
    assert_eq!(probe(&client, "s1", 1).await, 2);

    // Another tenant's key sees its own, empty, store: the same stream id
    // there is a stream of its own, starting at version 0.
    let other = h.connect(&zeta_token).await.unwrap();
    assert_eq!(count(&other).await.unwrap(), 0);
    assert_eq!(h.connected(&zeta), 1);
    assert_eq!(probe(&other, "s1", 0).await, 1);
    assert_eq!(
        count(&client).await.unwrap(),
        2,
        "acme's store is untouched"
    );
    drop(other);

    // Other tenants' traffic is no concern of the connection.
    for i in 0..4 {
        let filler = h.create(&format!("filler-{i}")).await;
        probe_as(&h, &filler, "f", 0).await;
    }
    assert_eq!(h.connected(&acme), 1, "the store is still connected");
    assert_eq!(count(&client).await.unwrap(), 2);

    // Disconnected: the registry forgets it.
    drop(client);
    h.wait_for("connection released", |h| h.connected(&acme) == 0)
        .await;

    // Reconnecting finds the events again.
    let again = h.connect(&acme_token).await.unwrap();
    assert_eq!(count(&again).await.unwrap(), 2);
    drop(again);
    h.finish().await;
}

#[tokio::test]
async fn bad_keys_and_suspended_tenants_are_refused() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;

    let err = h.refused(Some("tk_nobody-issued-this")).await;
    assert!(err.contains("unauthorized"), "{err}");
    let err = h.refused(None).await;
    assert!(err.contains("unauthorized"), "{err}");
    assert_eq!(
        h.connected(&acme),
        0,
        "a refused connection registers nothing"
    );

    h.platform.suspend_tenant(&acme.id, "unpaid").await.unwrap();
    let err = h.refused(Some(&token)).await;
    assert!(err.contains("suspended"), "{err}");

    h.platform.resume_tenant(&acme.id).await.unwrap();
    assert!(h.connect(&token).await.is_ok());
    h.finish().await;
}

#[tokio::test]
async fn revoking_a_key_ends_its_connections() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (token, key_id) = h.key(&acme).await;

    let client = h.connect(&token).await.unwrap();
    assert_eq!(probe(&client, "s1", 0).await, 1);
    assert_eq!(h.connections.count(&key_id), 1);

    h.platform
        .revoke_api_key("alice", &key_id, NOW + 1)
        .await
        .unwrap();
    assert_eq!(h.connections.close_key(&key_id), 1);
    assert_eq!(h.connections.count(&key_id), 0);

    // The connection is gone: requests fail, the reconnect is refused, and
    // the lease is released. A request sent while the client is being refused
    // waits for a connection that never comes, so it is bounded here rather
    // than by the client's timeout.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout(Duration::from_secs(2), count(&client)).await {
            Ok(Ok(_)) => assert!(tokio::time::Instant::now() < deadline, "still served"),
            Ok(Err(err)) => {
                let msg = format!("{err:#}");
                assert!(
                    msg.contains("connection lost") || msg.contains("timed out"),
                    "unexpected error: {msg}"
                );
                break;
            }
            Err(_) => break,
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The cut-off client keeps trying to reconnect in the background; drop it
    // so the checks below see only the server's state, not its retries.
    drop(client);
    h.wait_for("connection released", |h| h.connected(&acme) == 0)
        .await;
    assert_eq!(
        h.connections.count(&key_id),
        0,
        "a reconnect of the revoked key was accepted"
    );
    let err = h.refused(Some(&token)).await;
    assert!(err.contains("unauthorized"), "{err}");
    h.finish().await;
}

#[tokio::test]
async fn a_read_only_key_cannot_change_the_store() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (writer_token, _) = h.key(&acme).await;
    let (token, key_id) = h.key_with(&acme, SCOPE_STORE_READ).await;
    let writer = h.connect(&writer_token).await.unwrap();
    assert_eq!(probe(&writer, "s1", 0).await, 1);

    let client = h.connect(&token).await.unwrap();
    // It reads the catalog and nothing else: the tenant's own stream, where
    // the probe went, is not a storefront's business.
    assert_eq!(count(&client).await.unwrap(), 0, "the probe is not catalog");
    let merchant = evento::Evento::new(h.connect(&writer_token).await.unwrap());
    let mug = timada_catalog::Command::new(merchant.clone(), &acme.id)
        .create_product(
            "alice",
            timada_catalog::CreateProduct {
                name: "Mug".into(),
                slug: None,
            },
        )
        .await
        .unwrap();
    let product_type = <timada_catalog::aggregator::Product as evento::Aggregate>::aggregate_type();
    assert_eq!(
        count(&client).await.unwrap(),
        read_type(&writer, product_type).await.unwrap(),
        "it reads the catalog: every product event, nothing else"
    );
    assert!(count(&client).await.unwrap() >= 1);
    let refused = read_type(
        &client,
        <timada_tenancy::aggregator::Tenant as evento::Aggregate>::aggregate_type(),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{refused:#}").contains("may not read"),
        "{refused:#}"
    );
    let refused = timada_orders::Command::unscoped(evento::Evento::new(client.clone()))
        .load_cart(&mug.id)
        .await
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("may not read"),
        "a cart is not catalog: {refused:#}"
    );
    let total = count(&writer).await.unwrap();
    let err = evento::append("s1")
        .original_version(1)
        .event(&Renamed {
            name: "nope".into(),
        })
        .commit(&client)
        .await
        .unwrap_err();
    // Refused for the stream before the scope: a tenant's own stream is
    // not one it reads, so the version lookup the append starts with fails.
    let message = format!("{err:#}");
    assert!(
        message.contains("may not read") || message.contains("store:write"),
        "{message}"
    );
    assert_eq!(count(&writer).await.unwrap(), total, "nothing was written");

    // A snapshot it saves is dropped, not refused.
    client
        .save_snapshot(
            String::new(),
            "t".into(),
            "p".into(),
            "1".into(),
            "s1".into(),
            vec![1, 2, 3],
            Value("c".into()),
        )
        .await
        .unwrap();
    assert!(
        writer
            .get_snapshot(
                String::new(),
                "t".into(),
                "p".into(),
                "1".into(),
                "s1".into()
            )
            .await
            .unwrap()
            .is_none()
    );

    // Subscriber cursors are not served on a connection serving a tenant: a
    // storefront keeps its cursors in a store of its own, so no key can
    // fence out or move the dashboard's workers.
    let its_worker = ulid::Ulid::generate();
    let refused = client
        .upsert_subscriber("read-models".into(), its_worker)
        .await
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("subscriptions are not served"),
        "{refused:#}"
    );
    let refused = writer
        .upsert_subscriber("read-models".into(), its_worker)
        .await
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("subscriptions are not served"),
        "{refused:#}"
    );

    // Granted write, the key's connections end and the reconnect writes.
    h.platform
        .change_api_key_scopes("alice", &key_id, vec![SCOPE_STORE_WRITE.into()], NOW + 1)
        .await
        .unwrap();
    assert_eq!(h.connections.close_key(&key_id), 1);
    drop(client);
    let client = h.connect(&token).await.unwrap();
    assert_eq!(probe(&client, "s1", 1).await, total + 1);
    drop((client, writer, merchant));
    h.finish().await;
}

#[tokio::test]
async fn a_checkout_key_reads_how_the_shop_sells_and_never_a_cart_or_order() {
    let h = harness(None).await;
    let acme = h.create("acme").await;
    let (writer_token, _) = h.key(&acme).await;
    let (token, _) = h
        .key_with_scopes(&acme, &[SCOPE_STORE_READ, SCOPE_STORE_CHECKOUT])
        .await;
    let merchant = evento::Evento::new(h.connect(&writer_token).await.unwrap());
    let catalog = timada_catalog::Command::new(merchant.clone(), &acme.id);
    let mug = catalog
        .create_product(
            "alice",
            timada_catalog::CreateProduct {
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
    // The dashboard fills the cart for the storefront (`/store/carts/*`),
    // in the tenant, and says how the shop takes payment.
    let dashboard = timada_orders::Command::new(merchant.clone(), &acme.id);
    let cart_id = dashboard
        .create_cart(timada_orders::STOREFRONT_ACTOR)
        .await
        .unwrap();
    let item = timada_orders::CartItem {
        product_id: mug.id.clone(),
        variant_id: None,
        quantity: 2,
    };
    dashboard
        .add_to_cart(timada_orders::STOREFRONT_ACTOR, &cart_id, item.clone())
        .await
        .unwrap();
    dashboard
        .set_payment_method(
            "alice",
            Some(timada_orders::PaymentSetup {
                provider: "stripe".into(),
                publishable_key: "pk_test".into(),
            }),
        )
        .await
        .unwrap();

    let shop = evento::Evento::new(h.connect(&token).await.unwrap());
    let orders = timada_orders::Command::unscoped(shop.clone());
    // It reads how the shop sells...
    let setup = orders.payment_setup().await.unwrap().unwrap();
    assert_eq!(setup.publishable_key, "pk_test");
    assert!(orders.shipping_methods().await.unwrap().is_empty());
    // ...and never a cart or an order, not even one whose id it holds: the
    // dashboard answers for those, one at a time.
    let cart_type = <timada_orders::aggregator::Cart as evento::Aggregate>::aggregate_type();
    let order_type = <timada_orders::aggregator::Order as evento::Aggregate>::aggregate_type();
    // Nor an invoice series: every buyer's name and address are in it.
    let book_type = <timada_invoicing::aggregator::Book as evento::Aggregate>::aggregate_type();
    let invoicing = timada_invoicing::Command::unscoped(shop.clone());
    for refused in [
        format!("{:#}", orders.load_cart(&cart_id).await.unwrap_err()),
        format!(
            "{:#}",
            orders.load_order_of_cart(&cart_id).await.unwrap_err()
        ),
        format!("{:#}", read_type(&shop, cart_type).await.unwrap_err()),
        format!("{:#}", read_type(&shop, order_type).await.unwrap_err()),
        format!("{:#}", read_type(&shop, book_type).await.unwrap_err()),
        format!(
            "{:#}",
            invoicing
                .load_invoice_of_order("an-order")
                .await
                .unwrap_err()
        ),
    ] {
        assert!(refused.contains("may not read"), "{refused}");
    }
    // Nor does it write: filling a cart and asking to pay are the
    // dashboard's, which a storefront asks over HTTP.
    let refused = orders
        .create_cart(timada_orders::STOREFRONT_ACTOR)
        .await
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("store:write"),
        "{refused:#}"
    );
    let refused = orders
        .add_to_cart(timada_orders::STOREFRONT_ACTOR, &cart_id, item)
        .await
        .unwrap_err();
    let message = format!("{refused:#}");
    assert!(
        message.contains("may not read") || message.contains("store:write"),
        "{message}"
    );
    // Not the shop's shoppers' sign-in challenges, which the same store
    // holds.
    let refused = read_type(
        &shop,
        <timada_shoppers::aggregator::ShopperChallenge as evento::Aggregate>::aggregate_type(),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{refused:#}").contains("may not read"),
        "{refused:#}"
    );
    // A key without store:checkout reads the catalog, not how the shop
    // sells, and no cart either.
    let (reader_token, _) = h.key_with(&acme, SCOPE_STORE_READ).await;
    let reader = evento::Evento::new(h.connect(&reader_token).await.unwrap());
    let window = timada_orders::Command::unscoped(reader.clone());
    let refused = window.payment_setup().await.unwrap_err();
    assert!(
        format!("{refused:#}").contains("may not read"),
        "{refused:#}"
    );
    let refused = window.load_cart(&cart_id).await.unwrap_err();
    assert!(
        format!("{refused:#}").contains("may not read"),
        "{refused:#}"
    );
    drop(reader);

    // The merchant sees the cart as the dashboard filled it, and nothing
    // the storefront's key tried.
    let cart = dashboard.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(cart.item_count(), 2);
    drop((shop, merchant, orders, catalog, window));
    h.finish().await;
}

#[tokio::test]
async fn the_store_api_serves_over_tls() {
    let dir = std::env::temp_dir().join(format!("timada-store-tls-{}", ulid::Ulid::generate()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert, key, connector) = tls_files(&dir);
    let h = harness(Some((cert, key))).await;
    let acme = h.create("acme").await;
    let (token, _) = h.key(&acme).await;

    let client = Client::builder(h.addr)
        .token(&token)
        .tls(connector, ServerName::try_from("localhost").unwrap())
        .connect()
        .await
        .unwrap();
    assert_eq!(probe(&client, "s1", 0).await, 1);
    assert_eq!(h.connected(&acme), 1);

    // Plaintext does not get in.
    assert!(h.connect(&token).await.is_err());

    drop(client);
    h.finish().await;
    let _ = std::fs::remove_dir_all(&dir);
}
