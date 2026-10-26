//! The consensus node on three nodes in one process: a tenant's write lands
//! on every node in that tenant's streams only, a node that missed writes
//! catches up on its own, a node away longer than its peers wait for it is
//! told so and copies the store back at its next start, and a node at its
//! write backlog refuses writes.
//!
//! The dashboard is a binary crate, so the runtime is reached through the
//! binary's own test harness (`#[path]` includes of the modules under test),
//! which drags in items this test never calls.
#![allow(dead_code)]

#[path = "../src/cluster.rs"]
mod cluster;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/db.rs"]
mod db;
#[path = "../src/store_writer.rs"]
mod store_writer;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use timada_catalog::{CreateProduct, ProductState};
use timada_tenancy::{Command, CreateTenant, read_model_subscription};

use cluster::{Cluster, Executor, Files, StorePools};
use config::ClusterConfig;

struct Node {
    cluster: Arc<Cluster>,
    store: Executor,
    pool: SqlitePool,
    files: Files,
    dir: PathBuf,
    /// The tenant catalog's worker; dropping the handle would stop it.
    catalog: evento::subscription::Subscription,
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
    fn store_path(root: &Path, id: u64) -> PathBuf {
        root.join(format!("node{id}")).join("store.db")
    }

    /// Opens node `id` over its own files; a fresh node migrates them first.
    async fn start(root: &Path, id: u64, peers: &[SocketAddr], tuning: Tuning) -> Self {
        let dir = root.join(format!("node{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = Self::store_path(root, id);
        let files = Files {
            store_url: format!("sqlite:{}", path.display()),
            read_models_url: format!("sqlite:{}", dir.join("read-models.db").display()),
        };
        db::migrate(&files.store_url, db::store_migrator().unwrap())
            .await
            .unwrap();
        db::migrate(&files.read_models_url, db::read_model_migrator().unwrap())
            .await
            .unwrap();
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(3)
            .connect_with(options)
            .await
            .unwrap();
        let read_models = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(dir.join("read-models.db"))
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await
            .unwrap();
        let (cluster, store) = Cluster::open(
            Some(&ClusterConfig {
                node_id: id,
                peers: peers.to_vec(),
                listen: None,
                web_peers: Vec::new(),
                secret: None,
                absent_after_secs: tuning.absent_after_secs,
                write_backlog: tuning.write_backlog,
            }),
            None,
            &files,
            StorePools {
                read: pool.clone(),
                write: pool.clone(),
                cursors: pool.clone(),
            },
        )
        .await
        .unwrap();
        // The platform's catalog of tenants, in this node's read models.
        let catalog = read_model_subscription::<Executor>(read_models.clone())
            .start(&store)
            .await
            .unwrap();
        Self {
            cluster,
            store,
            pool: read_models,
            files,
            dir,
            catalog,
        }
    }

    /// Stops the node and closes its files.
    async fn stop(self) {
        self.catalog.stop();
        self.catalog.stopped().await;
        self.cluster.shutdown().await;
        self.pool.close().await;
    }

    fn catalog(&self, tenant: &timada_tenancy::TenantState) -> timada_catalog::Command<Executor> {
        timada_catalog::Command::new(self.store.clone(), &tenant.id)
    }
}

async fn free_addrs() -> Vec<SocketAddr> {
    let mut listeners = Vec::new();
    for _ in 0..3 {
        listeners.push(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
    }
    listeners.iter().map(|l| l.local_addr().unwrap()).collect()
}

async fn read_only(path: &Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(path).read_only(true))
        .await
        .unwrap()
}

/// Events of `tenant` (every tenant with `None`) in a node's store file.
async fn events_in(path: &Path, tenant: Option<&str>) -> i64 {
    if !path.exists() {
        return 0;
    }
    let pool = read_only(path).await;
    let count = match tenant {
        Some(tenant) => sqlx::query_scalar("SELECT COUNT(*) FROM event WHERE tenant = ?1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .unwrap(),
        None => sqlx::query_scalar("SELECT COUNT(*) FROM event")
            .fetch_one(&pool)
            .await
            .unwrap(),
    };
    pool.close().await;
    count
}

/// Consensus commands the journal still holds (not compacted).
async fn commands_in(path: &Path) -> i64 {
    let pool = read_only(path).await;
    let count = sqlx::query_scalar("SELECT COUNT(*) FROM accord_commands")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    count
}

async fn await_events(path: &Path, tenant: Option<&str>, expected: i64, what: &str) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while events_in(path, tenant).await < expected {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("{what}: {} events, expected {expected}", 0);
    });
}

async fn await_compacted(paths: &[PathBuf], what: &str) {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let mut done = true;
            for path in paths {
                done &= commands_in(path).await == 0;
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

/// Starts the three nodes together: each waits for a quorum of the others.
async fn start_all(root: &Path, addrs: &[SocketAddr], tuning: Tuning) -> [Node; 3] {
    let (a, b, c) = tokio::join!(
        Node::start(root, 0, addrs, tuning),
        Node::start(root, 1, addrs, tuning),
        Node::start(root, 2, addrs, tuning),
    );
    [a, b, c]
}

/// Creates a tenant through node 0 and waits for every node's catalog to
/// list it.
async fn create_everywhere(nodes: &[Node; 3], name: &str) -> timada_tenancy::TenantState {
    let tenant = Command(nodes[0].store.clone())
        .create_tenant(CreateTenant {
            name: name.into(),
            owner_user_id: "alice".into(),
        })
        .await
        .unwrap();
    for node in nodes {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
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
        .unwrap_or_else(|_| panic!("node {} never listed {name}", node.dir.display()));
    }
    tenant
}

fn temp_root(test: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("timada-cluster-{test}-{}", ulid::Ulid::generate()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

async fn create_product(
    node: &Node,
    tenant: &timada_tenancy::TenantState,
    name: &str,
) -> ProductState {
    node.catalog(tenant)
        .create_product(
            "alice",
            CreateProduct {
                name: name.into(),
                slug: None,
            },
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tenants_write_lands_on_every_node_in_its_own_streams() {
    let root = temp_root("streams");
    let addrs = free_addrs().await;
    let nodes = start_all(&root, &addrs, Tuning::default()).await;
    let acme = create_everywhere(&nodes, "acme").await;
    let zeta = create_everywhere(&nodes, "zeta").await;

    // The same product id in two tenants is two products.
    let mug = create_product(&nodes[0], &acme, "Mug").await;
    let zeta_cmd = nodes[1].catalog(&zeta);
    assert!(
        zeta_cmd.load_product(&mug.id).await.unwrap().is_none(),
        "zeta sees nothing of acme's product"
    );
    let mug_events = events_in(&Node::store_path(&root, 0), Some(&acme.id)).await;
    assert!(
        mug_events >= 2,
        "the slug claim and the creation: {mug_events}"
    );
    for id in 0..3 {
        await_events(
            &Node::store_path(&root, id),
            Some(&acme.id),
            mug_events,
            "acme's events reach every node",
        )
        .await;
        assert_eq!(
            events_in(&Node::store_path(&root, id), Some(&zeta.id)).await,
            0
        );
    }
    // Loaded in acme on another node: the same product.
    let seen = nodes[2].catalog(&acme).load_product(&mug.id).await.unwrap();
    assert_eq!(seen.map(|p| p.name), Some("Mug".to_owned()));

    for node in nodes {
        node.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_that_missed_writes_catches_up_when_it_returns() {
    let root = temp_root("catch-up");
    let addrs = free_addrs().await;
    let nodes = start_all(&root, &addrs, Tuning::default()).await;
    let acme = create_everywhere(&nodes, "acme").await;
    let [n0, n1, n2] = nodes;
    // Node 2 is down while the writes go through.
    n2.stop().await;
    let before = events_in(&Node::store_path(&root, 2), None).await;
    create_product(&n0, &acme, "Mug").await;
    create_product(&n1, &acme, "Plate").await;
    let on_0 = events_in(&Node::store_path(&root, 0), None).await;
    assert!(on_0 > before);

    // Node 2 comes back and gets what it missed, without anything asking.
    let n2 = Node::start(&root, 2, &addrs, Tuning::default()).await;
    await_events(
        &Node::store_path(&root, 2),
        None,
        on_0,
        "node 2 catches up on its own",
    )
    .await;
    for node in [n0, n1, n2] {
        node.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_away_too_long_copies_the_store_back_at_its_next_start() {
    let root = temp_root("rejoin");
    let addrs = free_addrs().await;
    let tuning = Tuning {
        absent_after_secs: 2,
        write_backlog: 100_000,
    };
    let nodes = start_all(&root, &addrs, tuning).await;
    let acme = create_everywhere(&nodes, "acme").await;
    let [n0, n1, n2] = nodes;
    // Node 2 goes down; the others write past its absence and tidy up.
    n2.stop().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    for i in 0..5 {
        create_product(&n0, &acme, &format!("Thing {i}")).await;
    }
    await_compacted(
        &[Node::store_path(&root, 0), Node::store_path(&root, 1)],
        "the two nodes tidy up without node 2",
    )
    .await;
    let on_0 = events_in(&Node::store_path(&root, 0), None).await;

    // Back with its old file: told to rejoin, it marks the store for a copy.
    let n2 = Node::start(&root, 2, &addrs, tuning).await;
    let marker = n2.files.marker().unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("node 2 is told to rejoin and marks its store");
    assert!(n2.cluster.health().rejoin_required);
    n2.stop().await;

    // The next start copies the store from a peer and serves again.
    let n2 = Node::start(&root, 2, &addrs, tuning).await;
    assert!(!marker.exists(), "the mark goes once the copy finished");
    assert!(n2.cluster.health().joined_at_start);
    await_events(
        &Node::store_path(&root, 2),
        None,
        on_0,
        "node 2 holds the copied store",
    )
    .await;
    let after = create_product(&n2, &acme, "After").await;
    assert_eq!(after.name, "After");
    for node in [n0, n1, n2] {
        node.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_at_its_write_backlog_refuses_writes() {
    let root = temp_root("backlog");
    let addrs = free_addrs().await;
    let tuning = Tuning {
        absent_after_secs: 0,
        write_backlog: 3,
    };
    let nodes = start_all(&root, &addrs, tuning).await;
    let acme = create_everywhere(&nodes, "acme").await;
    let [n0, n1, n2] = nodes;
    // Node 2 stays down for good: every write is kept for it.
    n2.stop().await;
    let mut refused = None;
    for i in 0..20 {
        match n0
            .catalog(&acme)
            .create_product(
                "alice",
                CreateProduct {
                    name: format!("Thing {i}"),
                    slug: None,
                },
            )
            .await
        {
            Ok(_) => {}
            Err(err) => {
                refused = Some(err);
                break;
            }
        }
    }
    let err = refused.expect("the backlog cap refuses a write");
    assert_eq!(
        timada_core::write_refusal(match &err {
            timada_catalog::CatalogError::Write(err) => err,
            other => panic!("not a write error: {other}"),
        }),
        Some(timada_core::WRITES_PAUSED),
        "{err}"
    );
    let (kept, limit) = n0.cluster.health().backlog;
    assert_eq!(limit, 3);
    assert!(kept >= 3, "{kept}");
    for node in [n0, n1] {
        node.stop().await;
    }
}

/// Many requests at once on one node, each loading then writing its own
/// streams: a version check must never see a version behind a commit that
/// returned (the pages load an aggregate right after writing it).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_writers_never_see_stale_versions() {
    let root = temp_root("stress");
    let files = Files {
        store_url: format!("sqlite:{}", root.join("store.db").display()),
        read_models_url: format!("sqlite:{}", root.join("read-models.db").display()),
    };
    db::migrate(&files.store_url, db::store_migrator().unwrap())
        .await
        .unwrap();
    db::migrate(&files.read_models_url, db::read_model_migrator().unwrap())
        .await
        .unwrap();
    let pools = StorePools {
        read: db::create_read_pool(&files.store_url, 3).await.unwrap(),
        write: db::create_write_pool(&files.store_url, db::Durability::Durable)
            .await
            .unwrap(),
        cursors: db::create_write_pool(&files.store_url, db::Durability::Relaxed)
            .await
            .unwrap(),
    };
    let (cluster, store) = Cluster::open(None, None, &files, pools).await.unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for t in 0..64 {
        let store = store.clone();
        tasks.spawn(async move {
            let catalog = timada_catalog::Command::new(store, format!("tenant-{t}"));
            for i in 0..6 {
                let product = catalog
                    .create_product(
                        "alice",
                        CreateProduct {
                            name: format!("Thing {i}"),
                            slug: None,
                        },
                    )
                    .await
                    .unwrap_or_else(|err| panic!("tenant {t} create {i}: {err}"));
                for j in 0..3 {
                    catalog
                        .rename_product("alice", &product.id, &format!("Thing {i}.{j}"))
                        .await
                        .unwrap_or_else(|err| panic!("tenant {t} rename {i}.{j}: {err}"));
                }
            }
        });
    }
    while let Some(done) = tasks.join_next().await {
        done.unwrap();
    }
    cluster.shutdown().await;
}
