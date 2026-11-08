//! The consensus node on three nodes in one process: a tenant's write lands
//! on every node in that tenant's streams only, a node that missed writes
//! catches up on its own, a node away longer than its peers wait for it is
//! told so and copies the store back at its next start, a node at its write
//! backlog refuses writes, the nodes up know who they are, and a key cut off
//! through one node loses its store connections on another.
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
#[path = "../src/read_only_store.rs"]
mod read_only_store;
#[path = "../src/store_api.rs"]
mod store_api;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use evento::metadata::Event;
use evento::remote::{Client, ServerHandle};
use evento::subscription::{Context, SubscriptionBuilder};
use timada_catalog::{CreateProduct, ProductState};
use timada_tenancy::aggregator::Created;
use timada_tenancy::{
    Command, CreateApiKey, CreateTenant, SCOPE_STORE_READ, SCOPE_STORE_WRITE,
    read_model_subscription,
};

use cluster::{Cluster, Executor};
use config::{ClusterConfig, StoreApiConfig};
use store_api::StoreConnections;

struct Node {
    cluster: Arc<Cluster>,
    store: Executor,
    /// The read models in the store's database.
    db: db::Db,
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
        root.join(format!("node{id}")).join("timada.db")
    }

    /// Opens node `id` over its own files; a fresh node migrates them first.
    async fn start(root: &Path, id: u64, peers: &[SocketAddr], tuning: Tuning) -> Self {
        Self::start_in(root, id, peers, tuning, None, &[]).await
    }

    /// [`start`](Self::start) for a node serving `region`, and doing the
    /// work of the regions of `also_serves` too.
    async fn start_in(
        root: &Path,
        id: u64,
        peers: &[SocketAddr],
        tuning: Tuning,
        region: Option<&str>,
        also_serves: &[&str],
    ) -> Self {
        let dir = root.join(format!("node{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = Self::store_path(root, id);
        let store = db::open_store(&path).await.unwrap();
        let store_db = db::Db::from(store.clone());
        let (cluster, store) = cluster::open(
            Some(&ClusterConfig {
                node_id: id,
                peers: peers.to_vec(),
                listen: None,
                web_peers: Vec::new(),
                secret: None,
                absent_after_secs: tuning.absent_after_secs,
                write_backlog: tuning.write_backlog,
            }),
            region,
            &also_serves
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            &path,
            store,
        )
        .await
        .unwrap();
        // The platform's catalog of tenants, in this node's read models.
        let catalog = read_model_subscription::<Executor>(store_db.clone())
            .start(&store)
            .await
            .unwrap();
        Self {
            cluster,
            store,
            db: store_db,
            dir,
            catalog,
        }
    }

    /// Stops the node and closes its files.
    async fn stop(self) {
        self.catalog.stop();
        self.catalog.stopped().await;
        self.cluster.shutdown().await;
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

/// One number out of a node's store file: Turso keeps one database per path
/// in a process, so this reads what the node (running or stopped) has.
async fn count_in(path: &Path, sql: &str, params: impl evento::turso::turso::IntoParams) -> i64 {
    let db = evento::turso::turso::Builder::new_local(path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    let mut rows = conn.query(sql, params).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
}

/// Events of `tenant` (every tenant with `None`) in a node's store file.
async fn events_in(path: &Path, tenant: Option<&str>) -> i64 {
    if !path.exists() {
        return 0;
    }
    match tenant {
        Some(tenant) => {
            count_in(
                path,
                "SELECT COUNT(*) FROM event WHERE tenant = ?1",
                (tenant,),
            )
            .await
        }
        None => count_in(path, "SELECT COUNT(*) FROM event", ()).await,
    }
}

/// Consensus commands the journal still holds (not compacted).
async fn commands_in(path: &Path) -> i64 {
    count_in(path, "SELECT COUNT(*) FROM cluster_commands", ()).await
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
                if timada_tenancy::active_tenant_ids(&node.db)
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
    let marker = cluster::rejoin_mark(&Node::store_path(&root, 2));
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

/// A store API on one node, with the subscription that ends its
/// connections when a key or a tenant is cut off on any node.
struct StoreApi {
    connections: Arc<StoreConnections>,
    addr: SocketAddr,
    server: ServerHandle,
    closing: evento::subscription::Subscription,
}

impl StoreApi {
    async fn start(node: &Node) -> Self {
        let connections = StoreConnections::new();
        // Before the listener, as `serve` starts it: no connection is older.
        let closing = store_api::closing_subscription::<Executor>(connections.clone())
            .start(&node.store)
            .await
            .unwrap();
        // The ephemeral port is only known from a listener.
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let config = StoreApiConfig {
            listen: addr,
            public_addr: None,
            tls_cert: None,
            tls_key: None,
        };
        let server = store_api::serve(Some(&config), node.store.clone(), connections.clone())
            .await
            .unwrap()
            .unwrap();
        Self {
            connections,
            addr,
            server,
            closing,
        }
    }

    /// Connects with `token`, once the node knows the key.
    async fn connect(&self, token: &str) -> Client {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            match Client::builder(self.addr).token(token).connect().await {
                Ok(client) => return client,
                Err(err) => assert!(
                    tokio::time::Instant::now() < deadline,
                    "never connected: {err:#}"
                ),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The error a new connection with `token` is refused with.
    async fn refused(&self, token: &str) -> String {
        match Client::builder(self.addr).token(token).connect().await {
            Ok(_) => panic!("the connection must be refused"),
            Err(err) => format!("{err:#}"),
        }
    }

    async fn stop(self) {
        self.closing.stop();
        self.closing.stopped().await;
        self.server.shutdown().await;
    }
}

/// Waits up to thirty seconds for `ok`.
async fn eventually(what: &str, mut ok: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !ok().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// What reading the tenant's own streams through `client` says: a key that
/// may write reads them, one that may only read is told it may not.
async fn reads_tenant_streams(client: &Client) -> Result<usize, String> {
    use evento::{Aggregate as _, Executor as _};
    let tenant = timada_tenancy::aggregator::Tenant::aggregate_type();
    client
        .read(
            Some(Arc::from(vec![evento::EventFilter {
                aggregate_type: tenant.to_owned(),
                aggregate_id: None,
                name: None,
            }])),
            None,
            evento::TenantFilter::All,
            evento::cursor::Args::forward(10, None),
            None,
        )
        .await
        .map(|page| page.edges.len())
        .map_err(|err| format!("{err:#}"))
}

/// A storefront connects to node 1 while the merchant and the operator work
/// on node 0: what they cut off there ends the connection on node 1, which
/// is checked only when a connection opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_cut_off_through_one_node_ends_its_connections_on_another() {
    let root = temp_root("store-api");
    let addrs = free_addrs().await;
    let nodes = start_all(&root, &addrs, Tuning::default()).await;
    let acme = create_everywhere(&nodes, "acme").await;
    let api = StoreApi::start(&nodes[1]).await;
    let platform = Command(nodes[0].store.clone());
    let new_key = async |scope: &str| {
        platform
            .create_api_key(
                "alice",
                CreateApiKey {
                    tenant_id: acme.id.clone(),
                    label: "Shop".into(),
                    scopes: vec![scope.to_owned()],
                },
                1_800_000_000,
            )
            .await
            .unwrap()
    };

    let writer = new_key(SCOPE_STORE_WRITE).await;
    let client = api.connect(&writer.token).await;
    assert_eq!(api.connections.count(&writer.key.id), 1);
    assert_eq!(reads_tenant_streams(&client).await, Ok(0));

    // Narrowed to reading on node 0: node 1 ends the connection, and the
    // store comes back under the new scopes.
    platform
        .change_api_key_scopes(
            "alice",
            &writer.key.id,
            vec![SCOPE_STORE_READ.into()],
            1_800_000_001,
        )
        .await
        .unwrap();
    eventually("the connection comes back read-only", async || {
        reads_tenant_streams(&client)
            .await
            .is_err_and(|err| err.contains("may not read"))
    })
    .await;

    // Revoked on node 0: node 1 ends the connection for good.
    platform
        .revoke_api_key("alice", &writer.key.id, 1_800_000_002)
        .await
        .unwrap();
    eventually("node 1 ends the revoked key's connection", async || {
        api.connections.count(&writer.key.id) == 0
    })
    .await;
    drop(client);
    let err = api.refused(&writer.token).await;
    assert!(err.contains("unauthorized"), "{err}");

    // Suspended on node 0: node 1 ends every connection of the tenant.
    let reader = new_key(SCOPE_STORE_READ).await;
    let client = api.connect(&reader.token).await;
    assert_eq!(api.connections.count_tenant(&acme.id), 1);
    platform.suspend_tenant(&acme.id, "unpaid").await.unwrap();
    eventually(
        "node 1 ends the suspended tenant's connections",
        async || api.connections.count_tenant(&acme.id) == 0,
    )
    .await;
    drop(client);
    let err = api.refused(&reader.token).await;
    assert!(err.contains("suspended"), "{err}");

    api.stop().await;
    for node in nodes {
        node.stop().await;
    }
}

/// Work spread over the nodes goes by who is up: idle nodes still hear from
/// each other, so each has its place among all three; one that stops is left
/// out once it has been silent long enough. A region's work is shared among
/// the nodes serving it, which each node learns from the others' gossip
/// (#442): node 0 serves `eu`, node 1 `us`, node 2 `eu` and also `us`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_nodes_up_share_out_what_they_do() {
    let root = temp_root("live");
    let addrs = free_addrs().await;
    let tuning = Tuning::default();
    let (n0, n1, n2) = tokio::join!(
        Node::start_in(&root, 0, &addrs, tuning, Some("eu"), &[]),
        Node::start_in(&root, 1, &addrs, tuning, Some("us"), &[]),
        Node::start_in(&root, 2, &addrs, tuning, Some("eu"), &["us"]),
    );
    let within = Duration::from_secs(5);
    // Under load a node may stall for a moment: each comes to see this, and a
    // node never heard from since it started would not.
    let sees = async |node: &Node, region: Option<&str>, live: (u64, u64)| {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while cluster::live_place(&node.cluster, within, region) != live {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{:?} for {region:?}, not {live:?}",
                cluster::live_place(&node.cluster, within, region)
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };

    // Nothing is written: gossip alone keeps them in sight of each other.
    tokio::time::sleep(within + Duration::from_secs(1)).await;
    sees(&n0, None, (0, 3)).await;
    sees(&n1, None, (1, 3)).await;
    sees(&n2, None, (2, 3)).await;
    // eu's work is nodes 0 and 2's, us's nodes 1 and 2's.
    sees(&n0, Some("eu"), (0, 2)).await;
    sees(&n2, Some("eu"), (1, 2)).await;
    sees(&n1, Some("us"), (0, 2)).await;
    sees(&n2, Some("us"), (1, 2)).await;

    n1.stop().await;
    sees(&n0, None, (0, 2)).await;
    sees(&n2, None, (1, 2)).await;
    // Node 2 does us's work alone now.
    sees(&n2, Some("us"), (0, 1)).await;
    sees(&n2, Some("eu"), (1, 2)).await;

    for node in [n0, n2] {
        node.stop().await;
    }
}

/// Many requests at once on one node, each loading then writing its own
/// streams: a version check must never see a version behind a commit that
/// returned (the pages load an aggregate right after writing it).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_writers_never_see_stale_versions() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let root = temp_root("stress");
    let path = root.join("timada.db");
    let store = db::open_store(&path).await.unwrap();
    let (cluster, store) = cluster::open(None, None, &[], &path, store).await.unwrap();
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

/// The tenants a node's saga saw, by name.
#[derive(Clone, Default)]
struct Seen(Arc<std::sync::Mutex<Vec<String>>>);

impl Seen {
    fn names(&self) -> Vec<String> {
        let mut names = self.0.lock().unwrap().clone();
        names.sort();
        names
    }
}

#[evento::subscription]
async fn on_created<E: evento::Executor>(
    context: &Context<'_, E>,
    event: Event<Created>,
) -> anyhow::Result<()> {
    let seen: Seen = context.extract();
    seen.0.lock().unwrap().push(event.data.name.clone());
    Ok(())
}

/// A saga as `serve` builds one: no routing key of its own, so it follows the
/// node's region; `.routing_key(r)` for a region the node also serves.
fn saga(seen: &Seen) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::new("test/created-saga")
        .data(seen.clone())
        .handler(on_created())
}

/// Decision 1 of the CQRS review: a saga runs on the nodes of the event's
/// region. Node 0 serves `eu`, node 1 `us`, node 2 `eu` and also `us` (the
/// `also_serves` copy, filtered on `us` with a cursor of its own). A tenant
/// created through node 0 is `eu`'s, one through node 1 `us`'s: node 0's saga
/// sees the first, node 1's the second, node 2's both, and nothing twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_saga_runs_on_the_nodes_of_its_region_and_where_it_is_also_served() {
    let root = temp_root("regions");
    let addrs = free_addrs().await;
    let tuning = Tuning::default();
    let (n0, n1, n2) = tokio::join!(
        Node::start_in(&root, 0, &addrs, tuning, Some("eu"), &[]),
        Node::start_in(&root, 1, &addrs, tuning, Some("us"), &[]),
        Node::start_in(&root, 2, &addrs, tuning, Some("eu"), &["us"]),
    );
    let nodes = [n0, n1, n2];
    let seen: [Seen; 3] = Default::default();
    let mut sagas = Vec::new();
    for (node, seen) in nodes.iter().zip(&seen) {
        sagas.push(saga(seen).start(&node.store).await.unwrap());
    }
    // Node 2 also serves `us`.
    sagas.push(
        saga(&seen[2])
            .routing_key("us")
            .start(&nodes[2].store)
            .await
            .unwrap(),
    );

    let acme = create_everywhere(&nodes, "acme").await;
    Command(nodes[1].store.clone())
        .create_tenant(CreateTenant {
            name: "globex".into(),
            owner_user_id: "bob".into(),
        })
        .await
        .unwrap();
    eventually("node 2 sees both regions' tenants", async || {
        seen[2].names() == ["acme", "globex"]
    })
    .await;
    // Enough passes for a wrongly routed event to have shown up.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(seen[0].names(), ["acme"], "node 0 serves eu alone");
    assert_eq!(seen[1].names(), ["globex"], "node 1 serves us alone");
    assert_eq!(
        seen[2].names(),
        ["acme", "globex"],
        "node 2 serves eu and us, once each"
    );
    assert_eq!(acme.name, "acme");

    for saga in sagas {
        saga.shutdown().await.unwrap();
    }
    for node in nodes {
        node.stop().await;
    }
}
