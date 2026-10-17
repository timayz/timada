//! The tenant runtime on three nodes in one process: a node that missed a
//! tenant's writes gets them from the catch-up pass, without anything opening
//! the tenant there, a node that lost a tenant's file copies it back from a
//! peer, and a node away longer than its peers wait for it no longer holds
//! their tidy-up back and copies the tenant back on return.
//!
//! The dashboard is a binary crate, so the runtime is reached through the
//! binary's own test harness (`#[path]` includes of the modules under test),
//! which drags in items this test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/limits.rs"]
mod limits;
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

/// The `[cluster]` knobs a test runs the nodes with.
#[derive(Clone, Copy)]
struct Tuning {
    /// `absent_after_secs`; 0 never gives a node up.
    absent_after_secs: u64,
    /// `write_backlog`.
    write_backlog: usize,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            absent_after_secs: 0,
            write_backlog: 100_000,
        }
    }
}

impl Node {
    async fn start(root: &Path, id: u64, peers: &[SocketAddr], tuning: Tuning) -> Self {
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
                absent_after_secs: tuning.absent_after_secs,
                write_backlog: tuning.write_backlog,
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
                max_open: Some(8),
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
    // the pass its copy stays behind. Its peers did not compact the writes
    // meanwhile: compaction waits for every node's watermark, so what node 2
    // missed is still there to send.
    let t2 = n2.tenants();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(t2.status(&tenant.id), TenantStatus::Stopped);
    assert_eq!(events_in(&t2.path_of(&tenant.id)).await, 0);
    for t in [&t0, &t1] {
        let path = t.path_of(&tenant.id);
        assert!(commands_in(&path).await > 0, "a peer compacted past node 2");
        assert_eq!(floor_of(&path).await, None);
    }

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
    // Caught up, it stays open until an eviction makes room.
    assert!(matches!(
        t2.status(&tenant.id),
        TenantStatus::Running { .. }
    ));

    t2.shutdown().await;
    t1.shutdown().await;
    t0.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A node whose tenant file is gone after its peers compacted the tenant
/// cannot get the compacted writes from the commands its peers still hold:
/// it copies the tenant from a peer instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_that_lost_a_tenant_file_copies_it_from_a_peer() {
    let root = std::env::temp_dir().join(format!("timada-cluster-{}", ulid::Ulid::generate()));
    let addrs = free_addrs().await;
    let nodes = futures_join(&root, &addrs).await;
    let [n0, n1, n2] = &nodes;
    let (t0, t1, t2) = (n0.tenants(), n1.tenants(), n2.tenants());
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
    t2.open(&tenant).await.unwrap();
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

    // Every node compacts the writes away.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let mut done = true;
            for t in [&t0, &t1, &t2] {
                let path = t.path_of(&tenant.id);
                done &= commands_in(&path).await == 0 && floor_of(&path).await.is_some();
            }
            if done {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("every node compacts the tenant");

    // Node 2 loses the file. The tenant is older than a fresh creation, so
    // opening it again copies it from a peer.
    t2.close(&tenant.id).await;
    let path = t2.path_of(&tenant.id);
    std::fs::remove_file(&path).unwrap();
    let mut known = tenant.clone();
    known.created_at -= 3600;
    tokio::time::timeout(Duration::from_secs(30), t2.open(&known))
        .await
        .expect("the copy ends")
        .unwrap();
    assert_eq!(events_in(&path).await, 3);
    assert!(floor_of(&path).await.is_some(), "the copied floor is kept");
    let mut marker = path.as_os_str().to_owned();
    marker.push(".joining");
    assert!(!PathBuf::from(marker).exists());

    // It takes part in the next write, which depends on compacted ones.
    evento::append("probe")
        .original_version(3)
        .event(&Renamed {
            name: "four".into(),
        })
        .commit(&executor)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while events_in(&path).await < 4 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the next write reaches node 2");

    t2.shutdown().await;
    t1.shutdown().await;
    t0.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

async fn read_only(path: &Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(path).read_only(true))
        .await
        .unwrap()
}

/// Consensus commands the tenant's journal still holds (not compacted).
async fn commands_in(path: &Path) -> i64 {
    let pool = read_only(path).await;
    let count = sqlx::query_scalar("SELECT COUNT(*) FROM accord_commands")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    count
}

/// The point below which the tenant's journal was compacted, if ever.
async fn floor_of(path: &Path) -> Option<evento_accord::Timestamp> {
    use evento_accord::Journal;
    let pool = read_only(path).await;
    let floor = evento_sql::SqlJournal::<sqlx::Sqlite>::new(pool.clone())
        .load_watermark()
        .await
        .unwrap();
    pool.close().await;
    floor
}

/// Starts the three nodes together: each waits for its platform group to
/// open, which needs a quorum of the others to be listening.
async fn futures_join(root: &Path, addrs: &[SocketAddr]) -> [Node; 3] {
    futures_join_with(root, addrs, Tuning::default()).await
}

async fn futures_join_with(root: &Path, addrs: &[SocketAddr], tuning: Tuning) -> [Node; 3] {
    let (a, b, c) = tokio::join!(
        Node::start(root, 0, addrs, tuning),
        Node::start(root, 1, addrs, tuning),
        Node::start(root, 2, addrs, tuning),
    );
    [a, b, c]
}

/// Creates a tenant through node 0 and waits for every node's catalog to
/// list it.
async fn create_everywhere(nodes: &[Node; 3]) -> timada_tenancy::TenantState {
    let tenant = Command(nodes[0].platform.clone())
        .create_tenant(CreateTenant {
            name: "acme".into(),
            owner_user_id: "alice".into(),
        })
        .await
        .unwrap();
    for node in nodes {
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
    tenant
}

async fn write(executor: &cluster::Executor, version: u16, name: &str) -> anyhow::Result<()> {
    evento::append("probe")
        .original_version(version)
        .event(&Renamed { name: name.into() })
        .commit(executor)
        .await?;
    Ok(())
}

/// Waits until the tenant's journal on each path holds no command and has a
/// floor: every one of those nodes tidied the writes up.
async fn await_compacted(paths: &[PathBuf], what: &str) {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let mut done = true;
            for path in paths {
                done &= commands_in(path).await == 0 && floor_of(path).await.is_some();
            }
            if done {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}: the nodes never tidied the writes up"));
}

async fn await_events(path: &Path, expected: i64, what: &str) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while events_in(path).await < expected {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}: {} events, expected {expected}", 0));
}

fn group_of_tenant(tenant: &timada_tenancy::TenantState) -> evento_accord::GroupId {
    evento_accord::GroupId(timada_tenancy::group_of(&tenant.id))
}

fn marker_of(path: &Path) -> PathBuf {
    let mut marker = path.as_os_str().to_owned();
    marker.push(".joining");
    PathBuf::from(marker)
}

/// A node away past `absent_after_secs` while writes go through no longer
/// holds the others' tidy-up back. Back with its old file, its copy is told
/// it fell behind, and the runtime copies the tenant back from a peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_away_too_long_no_longer_holds_tidy_up_back_and_copies_on_return() {
    let root = std::env::temp_dir().join(format!("timada-cluster-{}", ulid::Ulid::generate()));
    let addrs = free_addrs().await;
    let nodes = futures_join_with(
        &root,
        &addrs,
        Tuning {
            absent_after_secs: 2,
            ..Tuning::default()
        },
    )
    .await;
    let [n0, n1, n2] = &nodes;
    let (t0, t1, t2) = (n0.tenants(), n1.tenants(), n2.tenants());
    let tenant = create_everywhere(&nodes).await;
    t2.open(&tenant).await.unwrap();
    t1.open(&tenant).await.unwrap();
    let executor = t0.open(&tenant).await.unwrap();
    write(&executor, 0, "one").await.unwrap();
    let paths: Vec<PathBuf> = [&t0, &t1, &t2]
        .iter()
        .map(|t| t.path_of(&tenant.id))
        .collect();
    await_compacted(&paths, "baseline").await;

    // Node 2 goes down. The others keep writing and, past the limit, tidy
    // those writes up without it.
    t2.shutdown().await;
    for (version, name) in [(1, "two"), (2, "three"), (3, "four")] {
        write(&executor, version, name).await.unwrap();
    }
    await_compacted(&paths[..2], "with node 2 away").await;
    let group = group_of_tenant(&tenant);
    let peers = n0.cluster.group_peers(group);
    assert!(
        peers.iter().any(|p| p.id == 2 && p.given_up),
        "node 0 gave node 2 up: {peers:?}"
    );
    assert_eq!(events_in(&paths[2]).await, 1, "node 2 kept its old copy");

    // Node 2 comes back with that old copy: it is told it fell behind, and
    // the runtime copies the tenant back from a peer.
    let t2 = n2.tenants();
    t2.open(&tenant).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while t2.rejoins() == 0 || t2.status(&tenant.id) == TenantStatus::Stopped {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("node 2 copies the tenant back");
    await_events(&paths[2], 4, "the copy").await;
    assert!(!marker_of(&paths[2]).exists());
    assert_eq!(t2.rejoins(), 1);
    assert!(
        floor_of(&paths[2]).await.is_some(),
        "the copied floor is kept"
    );

    // Counted again: the next write lands on it and is tidied up everywhere.
    write(&executor, 4, "five").await.unwrap();
    await_events(&paths[2], 5, "the next write").await;
    await_compacted(&paths, "after the return").await;
    assert!(
        !n0.cluster.group_peers(group).iter().any(|p| p.given_up),
        "node 0 waits for node 2 again"
    );

    t2.shutdown().await;
    t1.shutdown().await;
    t0.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A node away past the limit while nothing was written lacks nothing: back,
/// it is waited for again and keeps its file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_back_before_its_peers_tidied_past_it_keeps_its_file() {
    let root = std::env::temp_dir().join(format!("timada-cluster-{}", ulid::Ulid::generate()));
    let addrs = free_addrs().await;
    let nodes = futures_join_with(
        &root,
        &addrs,
        Tuning {
            absent_after_secs: 2,
            ..Tuning::default()
        },
    )
    .await;
    let [n0, n1, n2] = &nodes;
    let (t0, t1, t2) = (n0.tenants(), n1.tenants(), n2.tenants());
    let tenant = create_everywhere(&nodes).await;
    t2.open(&tenant).await.unwrap();
    t1.open(&tenant).await.unwrap();
    let executor = t0.open(&tenant).await.unwrap();
    write(&executor, 0, "one").await.unwrap();
    let paths: Vec<PathBuf> = [&t0, &t1, &t2]
        .iter()
        .map(|t| t.path_of(&tenant.id))
        .collect();
    await_compacted(&paths, "baseline").await;

    t2.shutdown().await;
    let group = group_of_tenant(&tenant);
    tokio::time::timeout(Duration::from_secs(30), async {
        while !n0
            .cluster
            .group_peers(group)
            .iter()
            .any(|p| p.id == 2 && p.given_up)
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("node 0 gives node 2 up");

    let t2 = n2.tenants();
    t2.open(&tenant).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while n0.cluster.group_peers(group).iter().any(|p| p.given_up)
            || n1.cluster.group_peers(group).iter().any(|p| p.given_up)
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("node 2 is waited for again");
    assert_eq!(t2.rejoins(), 0, "nothing to copy back");
    assert!(!marker_of(&paths[2]).exists());

    write(&executor, 1, "two").await.unwrap();
    await_events(&paths[2], 2, "the next write").await;
    await_compacted(&paths, "after the return").await;
    assert_eq!(t2.rejoins(), 0);

    t2.shutdown().await;
    t1.shutdown().await;
    t0.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// With a node away and never given up, a tenant keeps every finished write
/// for it, up to `write_backlog`; at the limit it refuses new writes until
/// the node is back and has them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tenant_at_its_write_backlog_refuses_writes() {
    let root = std::env::temp_dir().join(format!("timada-cluster-{}", ulid::Ulid::generate()));
    let addrs = free_addrs().await;
    let nodes = futures_join_with(
        &root,
        &addrs,
        Tuning {
            absent_after_secs: 0,
            write_backlog: 3,
        },
    )
    .await;
    let [n0, n1, n2] = &nodes;
    let (t0, t1, t2) = (n0.tenants(), n1.tenants(), n2.tenants());
    let tenant = create_everywhere(&nodes).await;
    t2.open(&tenant).await.unwrap();
    t1.open(&tenant).await.unwrap();
    let executor = t0.open(&tenant).await.unwrap();
    let paths: Vec<PathBuf> = [&t0, &t1, &t2]
        .iter()
        .map(|t| t.path_of(&tenant.id))
        .collect();

    t2.shutdown().await;
    for (version, name) in [(0, "one"), (1, "two"), (2, "three")] {
        write(&executor, version, name).await.unwrap();
    }
    assert_eq!(t0.backlog(&tenant.id), Some((3, 3)));
    let err = write(&executor, 3, "four")
        .await
        .expect_err("the tenant refuses the write past its backlog");
    let refusal = err
        .downcast_ref::<evento::WriteError>()
        .and_then(timada_core::write_refusal);
    assert_eq!(
        refusal,
        Some(timada_core::WRITES_PAUSED),
        "a backlog at its cap is the `writes_paused` refusal: {err:#}"
    );
    assert!(
        format!("{err:#}").contains("backlog"),
        "unexpected error: {err:#}"
    );

    // Node 2 back and caught up, the writes are tidied up and the write goes.
    let t2 = n2.tenants();
    t2.open(&tenant).await.unwrap();
    await_events(&paths[2], 3, "catch-up").await;
    await_compacted(&paths, "after the return").await;
    write(&executor, 3, "four").await.unwrap();
    await_events(&paths[2], 4, "the write after").await;

    t2.shutdown().await;
    t1.shutdown().await;
    t0.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}
