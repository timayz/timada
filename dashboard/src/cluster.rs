//! The consensus host: every event database this node serves runs as its own
//! Accord group, all carried by one multiplexed transport.
//!
//! The platform store (identity, the tenant catalog, every read-model table)
//! is the default group over the two pools of `database.url`. Each tenant's
//! database is a group of its own, opened on demand by [`crate::tenants`].
//! Writes are coordinated through the cluster (strictly serializable,
//! leaderless; a quorum of `N = 2f+1` nodes must agree); reads, snapshots and
//! subscriptions are served from this node's own files. Without a `[cluster]`
//! section the node talks to itself over a loopback port, so the same code
//! serves both deployments.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use evento::Evento;
use evento_accord::{
    AccordExecutor, DataStore, ExecutorDataStore, GroupHost, GroupId, Journal, MuxTransport, Node,
    NodeConfig, NodeId, StaticTopology, SweepConfig, SweepScheduler, Topology,
};
use evento_sql::{RwSqlite, Sql, SqlJournal};
use sqlx::SqlitePool;
use tokio::task::JoinHandle;

use crate::config::ClusterConfig;
use crate::tenants::Tenants;

/// The platform store: Accord over a read/write pair of SQLite pools (reads
/// and subscription polling on the read pool, writes, replication and
/// acknowledgements on the single-connection write pool). Type-erased by
/// [`Evento`], which also carries the node's default routing key.
pub type Executor = Evento;

/// Stability margin subtracted from DB time to form the subscription watermark
/// (default 1s). Single-host SQLite, so it only needs to exceed the worst-case
/// duration of one event INSERT commit; subscription latency grows by about
/// this much.
pub const STABLE_MARGIN: Duration = Duration::from_millis(100);

/// Frames for a group this node has not opened yet wait this long for it.
const PENDING_FRAMES_PER_GROUP: usize = 64;
const PENDING_GROUPS: usize = 1024;
const PENDING_TTL: Duration = Duration::from_secs(10);

/// One per process: the transport, the sweep task and the group host.
pub struct Cluster {
    host: Arc<GroupHost>,
    ids: Vec<NodeId>,
    /// Shared by every group this node hosts.
    node_config: NodeConfig,
    /// The tenant runtime, once it exists: the transport's unrouted hook asks
    /// it to open a tenant a peer is already talking about.
    tenants: Arc<OnceLock<Arc<Tenants>>>,
    _tasks: Vec<JoinHandle<()>>,
}

impl Cluster {
    /// Binds the consensus listener and starts the host. Single node when
    /// `cluster` is `None`.
    pub async fn build(cluster: Option<&ClusterConfig>) -> Result<Self> {
        let (id, ids, peers, listener) = match cluster {
            None => {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .context("cannot bind the loopback consensus listener")?;
                let addr = listener.local_addr()?;
                let id = NodeId(0);
                tracing::info!(%addr, "single node");
                (id, vec![id], HashMap::from([(id, addr)]), listener)
            }
            Some(cluster) => {
                let id = NodeId(cluster.node_id);
                let ids: Vec<NodeId> = (0..cluster.peers.len() as u64).map(NodeId).collect();
                let peers: HashMap<NodeId, SocketAddr> = ids
                    .iter()
                    .copied()
                    .zip(cluster.peers.iter().copied())
                    .collect();
                let listen = cluster.listen_addr();
                let listener = tokio::net::TcpListener::bind(listen)
                    .await
                    .with_context(|| format!("cannot bind the consensus listener on {listen}"))?;
                tracing::info!(
                    node = cluster.node_id,
                    %listen,
                    size = cluster.peers.len(),
                    "cluster node"
                );
                (id, ids, peers, listener)
            }
        };

        let tenants: Arc<OnceLock<Arc<Tenants>>> = Arc::new(OnceLock::new());
        let hook = Arc::clone(&tenants);
        let mux = Arc::new(
            MuxTransport::new(id, peers)
                .with_pending_buffer(PENDING_FRAMES_PER_GROUP, PENDING_GROUPS, PENDING_TTL)
                .with_unrouted_handler(move |group| {
                    if let Some(tenants) = hook.get() {
                        tenants.reopen(group);
                    }
                }),
        );
        let serve = mux.serve(listener);
        let scheduler = SweepScheduler::new(SweepConfig::default());
        let sweep = scheduler.start();
        let host = Arc::new(GroupHost::new(mux, scheduler));

        // Pages load an aggregate right after writing it, possibly on another
        // node. With several nodes, a read-index barrier makes each
        // single-aggregate read wait for the writes a quorum already committed
        // (one consensus round per read). Alone, a node applies its own writes
        // before the commit returns, so the barrier would only add latency.
        let node_config = NodeConfig {
            linearizable_reads: ids.len() > 1,
            ..NodeConfig::default()
        };

        Ok(Self {
            host,
            ids,
            node_config,
            tenants,
            _tasks: vec![serve, sweep],
        })
    }

    /// Lets the transport open tenants on demand. Called once.
    pub fn attach_tenants(&self, tenants: Arc<Tenants>) {
        if self.tenants.set(tenants).is_err() {
            tracing::warn!("the tenant runtime was attached twice; keeping the first");
        }
    }

    fn topology(&self) -> Arc<dyn Topology> {
        Arc::new(StaticTopology::new(self.host.id(), self.ids.clone()))
    }

    /// Opens the platform store as the default group. `region`, when set, is
    /// the default routing key stamped on every new aggregate this node writes.
    pub async fn open_platform(
        &self,
        region: Option<&str>,
        read_pool: SqlitePool,
        write_pool: SqlitePool,
    ) -> Result<Executor> {
        // One `Rw` instance, cloned, for both the Accord data store and the
        // executor: a second `Sql` built from the same pool would not share the
        // write-wake channel that subscriptions rely on.
        let rw: RwSqlite = (
            Sql::from(read_pool).stable_margin(STABLE_MARGIN),
            Sql::from(write_pool.clone()).stable_margin(STABLE_MARGIN),
        )
            .into();
        let store = Arc::new(ExecutorDataStore::new(rw.clone())) as Arc<dyn DataStore>;
        // The consensus journal shares the single writer connection.
        let journal = Arc::new(SqlJournal::<sqlx::Sqlite>::new(write_pool)) as Arc<dyn Journal>;
        let node = self
            .host
            .open(
                GroupId::DEFAULT,
                self.topology(),
                store,
                journal,
                self.node_config,
            )
            .await
            .context("cannot open the platform store")?;

        let mut executor = Evento::new(AccordExecutor::<RwSqlite>::new(node, rw));
        if let Some(region) = region {
            tracing::info!(region, "default routing key");
            executor = executor.default_routing_key(region);
        }
        Ok(executor)
    }

    /// Opens a tenant's database as its own group. `sql` and `pool` wrap the
    /// same file: the journal shares the pool, the store applies to `sql`.
    pub async fn open_group(
        &self,
        group: GroupId,
        sql: Sql<sqlx::Sqlite>,
        pool: SqlitePool,
    ) -> Result<Node> {
        let store = Arc::new(ExecutorDataStore::new(sql)) as Arc<dyn DataStore>;
        let journal = Arc::new(SqlJournal::<sqlx::Sqlite>::new(pool)) as Arc<dyn Journal>;
        self.host
            .open(group, self.topology(), store, journal, self.node_config)
            .await
    }

    /// Stops a group's node; the caller closes its pool afterwards.
    pub fn close_group(&self, group: GroupId) {
        self.host.close(group);
    }
}
