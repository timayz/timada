//! The tenant runtime on three nodes in one process: a node that missed a
//! tenant's writes gets them from the catch-up pass, without anything opening
//! the tenant there.
//!
//! The dashboard is a binary crate, so the runtime is reached through the
//! binary's own test harness (`#[path]` includes of the modules under test),
//! which drags in items this test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/tenants.rs"]
mod tenants;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use evento::migrator::Migrate;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_tenancy::aggregator::Renamed;
use timada_tenancy::{Command, CreateTenant, read_model_subscription};

use cluster::{Cluster, Executor};
use config::{ClusterConfig, TenantsConfig};
use tenants::{CatchUp, TenantStatus, Tenants};

struct Node {
    cluster: Arc<Cluster>,
    platform: Executor,
    pool: SqlitePool,
    dir: PathBuf,
}

impl Node {
    async fn start(root: &Path, id: u64, peers: &[SocketAddr]) -> Self {
        let dir = root.join(format!("node{id}"));
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
        let cluster = Arc::new(
            Cluster::build(Some(&ClusterConfig {
                node_id: id,
                peers: peers.to_vec(),
                listen: None,
                web_peers: Vec::new(),
                secret: None,
            }))
            .await
            .unwrap(),
        );
        let platform = cluster
            .open_platform(None, pool.clone(), pool.clone())
            .await
            .unwrap();
        Self {
            cluster,
            platform,
            pool,
            dir,
        }
    }

    /// The tenant runtime; until it exists, frames for a tenant's group are
    /// ignored here, as on a node that is down.
    fn tenants(&self) -> Arc<Tenants> {
        Tenants::new(
            self.cluster.clone(),
            &TenantsConfig {
                dir: self.dir.join("tenants"),
                idle_timeout_secs: 60,
                max_open: 8,
            },
            None,
            self.platform.clone(),
            self.pool.clone(),
            Vec::new(),
        )
    }

    /// Brings the catalog read model up to date with the platform store.
    async fn read_catalog(&self) {
        read_model_subscription::<Executor>(self.pool.clone())
            .run_once(&self.platform)
            .await
            .unwrap();
    }
}

/// Three free local ports, for the nodes to know each other before binding.
async fn free_addrs() -> Vec<SocketAddr> {
    let mut listeners = Vec::new();
    for _ in 0..3 {
        listeners.push(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
    }
    listeners.iter().map(|l| l.local_addr().unwrap()).collect()
}

async fn events_in(path: &Path) -> i64 {
    if !path.exists() {
        return 0;
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(path).read_only(true))
        .await
        .unwrap();
    let count = sqlx::query_scalar("SELECT COUNT(*) FROM event")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    count
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_that_missed_writes_catches_up_without_a_request() {
    let root = std::env::temp_dir().join(format!("timada-cluster-{}", ulid::Ulid::generate()));
    let addrs = free_addrs().await;
    let nodes = futures_join(&root, &addrs).await;
    let [n0, n1, n2] = &nodes;

    // Nodes 0 and 1 run the tenant; node 2 has no tenant runtime yet, so it
    // takes no part in the tenant's writes (two of three are a quorum).
    let t0 = n0.tenants();
    let t1 = n1.tenants();
    let tenant = Command(n0.platform.clone())
        .create_tenant(CreateTenant {
            name: "acme".into(),
            owner_user_id: "alice".into(),
        })
        .await
        .unwrap();
    for node in &nodes {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                node.read_catalog().await;
                if timada_tenancy::active_tenant_ids(&node.pool)
                    .await
                    .unwrap()
                    .contains(&tenant.id)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the catalog reaches every node");
    }
    t1.open(&tenant).await.unwrap();
    let executor = t0.open(&tenant).await.unwrap();
    for (version, name) in ["one", "two", "three"].into_iter().enumerate() {
        evento::append("probe")
            .original_version(version as u16)
            .event(&Renamed { name: name.into() })
            .commit(&executor)
            .await
            .unwrap();
    }

    // Node 2 comes back. Sweep gossip alone never opens a tenant, so without
    // the pass its copy stays behind.
    let t2 = n2.tenants();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(t2.status(&tenant.id), TenantStatus::Stopped);
    assert_eq!(events_in(&t2.path_of(&tenant.id)).await, 0);

    let pass = t2
        .start_catch_up(CatchUp {
            min_hold: Duration::from_secs(2),
            quiet: Duration::from_millis(500),
            max_hold: Duration::from_secs(20),
            parallel: 2,
        })
        .expect("a node with peers catches up");
    tokio::time::timeout(Duration::from_secs(60), pass)
        .await
        .expect("the pass ends")
        .unwrap();

    assert_eq!(events_in(&t2.path_of(&tenant.id)).await, 3);
    // The writes it fetched are younger than the idle timeout: the tenant is
    // in use in the cluster, so it stays open, as the sweeper would keep it.
    assert!(matches!(
        t2.status(&tenant.id),
        TenantStatus::Running { .. }
    ));

    t2.shutdown().await;
    t1.shutdown().await;
    t0.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// Starts the three nodes together: each waits for its platform group to
/// open, which needs a quorum of the others to be listening.
async fn futures_join(root: &Path, addrs: &[SocketAddr]) -> [Node; 3] {
    let (a, b, c) = tokio::join!(
        Node::start(root, 0, addrs),
        Node::start(root, 1, addrs),
        Node::start(root, 2, addrs),
    );
    [a, b, c]
}
