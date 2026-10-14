//! The store API end to end on one node: a storefront connects with a key
//! and is served its tenant's store only; the connection leases the tenant;
//! a revoked key is cut off; a suspended tenant and bad keys are refused;
//! the same over TLS.
//!
//! The dashboard is a binary crate, so the modules are reached through the
//! binary's own test harness (`#[path]` includes), which drags in items this
//! test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/limits.rs"]
mod limits;
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

use evento::Executor as _;
use evento::cursor::{Args, Value};
use evento::migrator::Migrate;
use evento::remote::{Client, ServerHandle};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_tenancy::aggregator::Renamed;
use timada_tenancy::{
    Command, CreateApiKey, CreateTenant, SCOPE_STORE_READ, SCOPE_STORE_WRITE, TenantState,
    read_model_subscription,
};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
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
    let tenants = Tenants::new(
        cluster,
        &TenantsConfig {
            dir: dir.join("tenants"),
            max_open: Some(4),
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
        self.key_with(tenant, SCOPE_STORE_WRITE).await
    }

    async fn key_with(&self, tenant: &TenantState, scope: &str) -> (String, String) {
        let created = self
            .platform
            .create_api_key(
                "alice",
                CreateApiKey {
                    tenant_id: tenant.id.clone(),
                    label: "Shop".into(),
                    scopes: vec![scope.into()],
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

async fn count(client: &Client) -> anyhow::Result<usize> {
    Ok(client
        .read(None, None, Args::forward(100, None), None)
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
    assert_eq!(h.leases(&acme), None, "nothing open before a connection");

    let client = h.connect(&acme_token).await.unwrap();
    assert_eq!(h.leases(&acme), Some(1));
    assert_eq!(probe(&client, "s1", 0).await, 1);
    assert_eq!(probe(&client, "s1", 1).await, 2);

    // Another tenant's key sees its own, empty, store.
    let other = h.connect(&zeta_token).await.unwrap();
    assert_eq!(count(&other).await.unwrap(), 0);
    assert_eq!(h.leases(&zeta), Some(1));
    drop(other);

    // Leased: filling the node with other tenants leaves it open.
    let mut fillers = Vec::new();
    for i in 0..4 {
        fillers.push(h.create(&format!("filler-{i}")).await);
    }
    for filler in &fillers {
        h.tenants.open(filler).await.unwrap();
    }
    assert_eq!(
        h.leases(&acme),
        Some(1),
        "a connected store keeps its tenant open"
    );
    assert_eq!(count(&client).await.unwrap(), 2);

    // Disconnected: the lease goes, and the next tenants opened evict it.
    drop(client);
    h.wait_for("lease released", |h| h.leases(&acme) == Some(0))
        .await;
    for filler in &fillers {
        h.tenants.open(filler).await.unwrap();
    }
    assert_eq!(h.leases(&acme), None);

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
    assert_eq!(h.leases(&acme), None, "a refused connection opens nothing");

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
                assert!(msg.contains("connection lost"), "unexpected error: {msg}");
                break;
            }
            Err(_) => break,
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.wait_for("lease released", |h| h.leases(&acme) == Some(0))
        .await;
    let err = h.refused(Some(&token)).await;
    assert!(err.contains("unauthorized"), "{err}");
    drop(client);
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
    assert_eq!(count(&client).await.unwrap(), 1, "it reads");
    let err = evento::append("s1")
        .original_version(1)
        .event(&Renamed {
            name: "nope".into(),
        })
        .commit(&client)
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("store:write"), "{err}");
    assert_eq!(count(&writer).await.unwrap(), 1, "nothing was written");

    // A snapshot it saves is dropped, not refused.
    client
        .save_snapshot(
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
            .get_snapshot("t".into(), "p".into(), "1".into(), "s1".into())
            .await
            .unwrap()
            .is_none()
    );

    // Its subscriber cursors are its own: the tenant's worker of the same
    // name is not fenced out.
    let tenant_worker = ulid::Ulid::generate();
    writer
        .upsert_subscriber("read-models".into(), tenant_worker)
        .await
        .unwrap();
    let its_worker = ulid::Ulid::generate();
    client
        .upsert_subscriber("read-models".into(), its_worker)
        .await
        .unwrap();
    assert!(
        client
            .is_subscriber_running("read-models".into(), its_worker)
            .await
            .unwrap()
    );
    assert!(
        writer
            .is_subscriber_running("read-models".into(), tenant_worker)
            .await
            .unwrap()
    );

    // Granted write, the key's connections end and the reconnect writes.
    h.platform
        .change_api_key_scopes("alice", &key_id, vec![SCOPE_STORE_WRITE.into()], NOW + 1)
        .await
        .unwrap();
    assert_eq!(h.connections.close_key(&key_id), 1);
    drop(client);
    let client = h.connect(&token).await.unwrap();
    assert_eq!(probe(&client, "s1", 1).await, 2);
    drop((client, writer));
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
    assert_eq!(h.leases(&acme), Some(1));

    // Plaintext does not get in.
    assert!(h.connect(&token).await.is_err());

    drop(client);
    h.finish().await;
    let _ = std::fs::remove_dir_all(&dir);
}
