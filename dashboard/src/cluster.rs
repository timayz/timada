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
use std::sync::{Arc, Mutex};
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

/// How old an unapplied transaction must be before a node takes it over
/// from its coordinator; see [`Cluster::build`].
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(3);

/// Frames for a group this node has not opened yet wait this long for it.
const PENDING_FRAMES_PER_GROUP: usize = 64;
const PENDING_GROUPS: usize = 1024;
pub(crate) const PENDING_TTL: Duration = Duration::from_secs(10);

/// One per process: the transport, the sweep task and the group host.
pub struct Cluster {
    host: Arc<GroupHost>,
    ids: Vec<NodeId>,
    /// Shared by every group this node hosts.
    node_config: NodeConfig,
    /// The tenant runtime, once it exists: the transport's unrouted hook asks
    /// it to open a tenant a peer is already talking about, and the host's
    /// rejoin hook asks it to copy a tenant this node fell behind on.
    tenants: Arc<Mutex<Option<Arc<Tenants>>>>,
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

        let tenants: Arc<Mutex<Option<Arc<Tenants>>>> = Arc::new(Mutex::new(None));
        let hook = Arc::clone(&tenants);
        let mux = Arc::new(
            MuxTransport::new(id, peers)
                .with_pending_buffer(PENDING_FRAMES_PER_GROUP, PENDING_GROUPS, PENDING_TTL)
                .with_unrouted_handler(move |group| {
                    if let Some(tenants) = attached(&hook) {
                        tenants.reopen(group);
                    }
                }),
        );
        let serve = mux.serve(listener);
        let scheduler = SweepScheduler::new(SweepConfig::default());
        let sweep = scheduler.start();
        let hook = Arc::clone(&tenants);
        let host = Arc::new(
            GroupHost::new(mux, scheduler).with_rejoin_handler(move |group| {
                if let Some(tenants) = attached(&hook) {
                    tenants.rejoin_from_peers(group);
                }
            }),
        );

        // Pages load an aggregate right after writing it, possibly on another
        // node. With several nodes, a read-index barrier makes each
        // single-aggregate read wait for the writes a quorum already committed
        // (one consensus round per read). Alone, a node applies its own writes
        // before the commit returns, so the barrier would only add latency.
        //
        // A peer opens a tenant's group only once frames for it arrive (see
        // `Tenants::reopen`), which can take a second when the tenant is new.
        // The frames wait meanwhile, and a node takes any transaction older than
        // `recovery_timeout` for one whose coordinator died: with the default
        // (300 ms) the late peer would take over a write its coordinator is
        // still running, and the coordinator would answer that write as failed
        // even though recovery then commits it. Three seconds outlasts the
        // reopen; a coordinator that really died is recovered that much later.
        // Alone, a node has no peer to open late and keeps the default.
        //
        // A node that stays down holds every finished write on the others
        // until it returns (they compact only what every node has applied),
        // so past `absent_after` the others stop waiting for it; a node away
        // that long copies each tenant it fell behind on back from a peer
        // (`Tenants::rejoin_from_peers`). `write_backlog` caps what a tenant
        // keeps meanwhile. Alone, a node has no one to wait for.
        let defaults = NodeConfig::default();
        let node_config = NodeConfig {
            linearizable_reads: ids.len() > 1,
            recovery_timeout: if ids.len() > 1 {
                RECOVERY_TIMEOUT
            } else {
                defaults.recovery_timeout
            },
            peer_absence_limit: match cluster {
                Some(cluster) if ids.len() > 1 => cluster.absent_after(),
                _ => None,
            },
            max_commands: cluster.map_or(defaults.max_commands, |cluster| cluster.write_backlog),
            ..defaults
        };

        Ok(Self {
            host,
            ids,
            node_config,
            tenants,
            _tasks: vec![serve, sweep],
        })
    }

    /// Lets the transport open tenants on demand, and the host copy one back.
    /// A runtime attached later replaces the one before it (a test restarts
    /// a node's runtime; a server attaches one).
    pub fn attach_tenants(&self, tenants: Arc<Tenants>) {
        let mut attached = self.tenants.lock().expect("tenants poisoned");
        if attached.is_some() {
            tracing::debug!("the tenant runtime was attached again; using the new one");
        }
        *attached = Some(tenants);
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
    /// With `join`, the file is new on a node whose peers may already have
    /// compacted the group: it is filled from a peer's copy before the group
    /// runs, instead of from the commands the peers still hold, which would
    /// leave out everything compacted. Without peers there is nothing to
    /// join and it opens as usual.
    pub async fn open_group(
        &self,
        group: GroupId,
        sql: Sql<sqlx::Sqlite>,
        pool: SqlitePool,
        join: bool,
    ) -> Result<Node> {
        let store = Arc::new(ExecutorDataStore::new(sql)) as Arc<dyn DataStore>;
        let journal = Arc::new(SqlJournal::<sqlx::Sqlite>::new(pool)) as Arc<dyn Journal>;
        if join && self.has_peers() {
            return self
                .host
                .open_joining(
                    group,
                    self.topology(),
                    store,
                    journal,
                    self.node_config,
                    &self.contacts(),
                )
                .await;
        }
        self.host
            .open(group, self.topology(), store, journal, self.node_config)
            .await
    }

    /// The other nodes, starting with the one after this node, so the nodes
    /// of a cluster do not all ask the same peer first.
    fn contacts(&self) -> Vec<NodeId> {
        let me = self.host.id();
        let at = self.ids.iter().position(|&id| id == me).unwrap_or(0);
        self.ids[at + 1..]
            .iter()
            .chain(&self.ids[..at])
            .copied()
            .collect()
    }

    /// Stops a group's node; the caller closes its pool afterwards.
    pub fn close_group(&self, group: GroupId) {
        self.host.close(group);
    }

    /// Whether the cluster has other nodes to catch up from.
    pub fn has_peers(&self) -> bool {
        self.ids.len() > 1
    }

    /// A counter that moves whenever the group's applied state advances
    /// (a write applied here, or events imported from a peer); `None` when the
    /// group is not open on this node.
    pub fn group_activity(&self, group: GroupId) -> Option<u64> {
        self.host
            .get(group)
            .map(|node| node.sweep_pressure().activity)
    }

    /// Whether a group has a transaction in flight or applied within the
    /// last moment (its node still holds commands compaction has not dropped
    /// yet). On a node that only takes part in another node's writes, this is
    /// the sign the group is in use.
    pub fn group_busy(&self, group: GroupId) -> bool {
        self.host
            .get(group)
            .is_some_and(|node| node.command_count() > 0)
    }

    /// The finished writes a group still keeps for nodes that are away, and
    /// the most it may keep before refusing new ones; `None` when the group
    /// is not open here.
    pub fn group_backlog(&self, group: GroupId) -> Option<(usize, usize)> {
        self.host
            .get(group)
            .map(|node| (node.command_count(), node.config().max_commands))
    }

    /// What this node knows of the other nodes on a group: empty alone or
    /// when the group is not open here.
    pub fn group_peers(&self, group: GroupId) -> Vec<PeerState> {
        let Some(node) = self.host.get(group) else {
            return Vec::new();
        };
        let absent = node.absent_peers();
        self.ids
            .iter()
            .filter(|&&id| id != node.id())
            .map(|&id| PeerState {
                id: id.0,
                silent_for: node.silent_for(id),
                given_up: absent.contains(&id),
            })
            .collect()
    }
}

/// Another node, as seen from one group on this node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerState {
    /// Its index into `cluster.peers`.
    pub id: u64,
    /// How long since it was last heard from on the group (since the group
    /// opened here, if never).
    pub silent_for: Duration,
    /// Whether this node stopped waiting for it (`cluster.absent_after_secs`).
    pub given_up: bool,
}

/// The attached tenant runtime, if any. A hook runs on the transport's or the
/// host's task: it takes the lock briefly and never calls back into them.
fn attached(slot: &Mutex<Option<Arc<Tenants>>>) -> Option<Arc<Tenants>> {
    slot.lock().expect("tenants poisoned").clone()
}
