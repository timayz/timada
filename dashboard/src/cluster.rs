//! The consensus node: the one Accord node this process runs, over the store.
//!
//! The store (every tenant's events, the snapshots, the subscription cursors
//! and the consensus journal) is the two pools of `database.store`. Writes
//! are coordinated through the cluster (strictly serializable, leaderless; a
//! quorum of `N = 2f+1` nodes must agree); reads, snapshots and subscriptions
//! are served from this node's own file. Without a `[cluster]` section the
//! node talks to itself in memory, so the same code serves both deployments.
//!
//! A node that was away longer than `cluster.absent_after_secs` while writes
//! went through is told so when it returns: its peers tidied those writes up
//! without it and cannot send them any more. The node then refuses writes
//! (`writes_paused` on the pages) and marks its store for a copy; the next
//! `serve` empties the store, keeps the journal, and opens the node by
//! joining, which copies a peer's store page by page. The `rejoin` command
//! sets the same mark by hand, for a store that was lost.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use evento::Evento;
use evento::Rw;
use evento::accord::{
    AccordExecutor, DataStore, ExecutorDataStore, HybridLogicalClock, InMemoryNetwork, Journal,
    MessageSink, Node, NodeConfig, NodeId, PartialSnapshot, StaticTopology, TcpTransport, Topology,
    serve,
};
use evento::sql::{Sql, SqlJournal, Sqlite};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::ClusterConfig;
use crate::store_writer::StoreWriter;

/// The store: Accord over the read pool, the durable write connection
/// (events and the consensus journal) and the relaxed one (subscription
/// cursors and snapshots), see [`StorePools`]. Type-erased by [`Evento`],
/// which also carries the node's default routing key.
pub type Executor = Evento;

/// The store's local executor: reads on the read pool, writes split by what
/// they must promise.
type Local = Rw<Sqlite, StoreWriter>;

/// The store's pools, all on `database.store`.
#[derive(Clone)]
pub struct StorePools {
    /// Read-only, sized by `database.max_connections`.
    pub read: SqlitePool,
    /// One connection, `synchronous = FULL`: the events the node applies
    /// and the consensus journal, which share it so an apply is on disk
    /// before the journal records it.
    pub write: SqlitePool,
    /// One connection, `synchronous = NORMAL`: the subscription cursors and
    /// the snapshots. Every subscription acknowledges each event it takes;
    /// an fsync apiece on the write connection starved the node's journal
    /// flushes under load.
    pub cursors: SqlitePool,
}

/// Stability margin subtracted from DB time to form the subscription watermark
/// (default 1s). Single-host SQLite, so it only needs to exceed the worst-case
/// duration of one event INSERT commit; subscription latency grows by about
/// this much.
pub const STABLE_MARGIN: Duration = Duration::from_millis(100);

/// Frames waiting for this node's inbox loop.
const INBOX_FRAMES: usize = 1024;

/// What `serve` needs to know about the store and the read models to open the
/// node: where the files are, so a copy from a peer can empty them first.
#[derive(Debug, Clone)]
pub struct Files {
    /// `database.store`, a sqlx DSN.
    pub store_url: String,
    /// `database.read_models`, a sqlx DSN.
    pub read_models_url: String,
}

impl Files {
    /// The file behind the store DSN.
    pub fn store_path(&self) -> Result<PathBuf> {
        Ok(SqliteConnectOptions::from_str(&self.store_url)?
            .get_filename()
            .to_path_buf())
    }

    /// The mark that makes the next open copy the store from a peer.
    pub fn marker(&self) -> Result<PathBuf> {
        Ok(joining_marker(&self.store_path()?))
    }
}

/// `<store file>.joining`: present from the moment a copy is asked for until
/// one finished.
fn joining_marker(store: &Path) -> PathBuf {
    let mut marker = store.as_os_str().to_owned();
    marker.push(".joining");
    PathBuf::from(marker)
}

/// Asks the next open to copy the store from a peer: the mark is written,
/// nothing else happens until `serve` starts.
pub fn mark_for_rejoin(files: &Files) -> Result<PathBuf> {
    let marker = files.marker()?;
    if let Some(parent) = marker.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&marker, b"").with_context(|| format!("cannot write {}", marker.display()))?;
    Ok(marker)
}

/// One per process: the node, its tasks and what the pages show of it.
pub struct Cluster {
    node: Node,
    ids: Vec<NodeId>,
    /// The inbox loop, the recovery sweep, the listener and the signal watch.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Whether this start copied the store from a peer.
    joined: bool,
    /// Set once the node asked for a copy (told to rejoin, or its journal
    /// found rolled back): the pages say so, the next `serve` copies.
    copy_pending: Arc<AtomicBool>,
}

impl Cluster {
    /// Opens the node over the store pools and starts it. Single node when
    /// `cluster` is `None`. `region`, when set, is the default routing key
    /// stamped on every new aggregate this node writes.
    ///
    /// When the store is marked for a copy (see [`mark_for_rejoin`]) and the
    /// node has peers, the store is emptied (the journal is kept), the
    /// read-model database is recreated, and the node opens by joining: a
    /// peer's store is installed page by page before anything is served.
    /// The mark stays when the copy fails, so the next start tries again.
    pub async fn open(
        cluster: Option<&ClusterConfig>,
        region: Option<&str>,
        files: &Files,
        pools: StorePools,
    ) -> Result<(Arc<Self>, Executor)> {
        let StorePools {
            read: read_pool,
            write: write_pool,
            cursors: cursor_pool,
        } = pools;
        let (id, ids, peers, listener) = match cluster {
            None => {
                let id = NodeId(0);
                tracing::info!("single node");
                (id, vec![id], HashMap::new(), None)
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
                (id, ids, peers, Some(listener))
            }
        };
        let has_peers = ids.len() > 1;

        let marker = files.marker()?;
        let joining = marker.exists();
        if joining && !has_peers {
            anyhow::bail!(
                "{} asks for a copy of the store, but a single node has no peer to copy it from; remove the file to open the store as it is",
                marker.display()
            );
        }
        if joining {
            tracing::warn!(marker = %marker.display(), "the store is to be copied from a peer");
            crate::db::empty_store(&write_pool)
                .await
                .context("cannot empty the store before copying it")?;
            crate::db::recreate_read_models(&files.read_models_url)
                .await
                .context("cannot recreate the read models before copying the store")?;
        }

        // One `Rw` instance, cloned, for both the Accord data store and the
        // executor: a second `Sql` built from the same pool would not share the
        // write-wake channel that subscriptions rely on.
        let rw: Local = (
            Sql::from(read_pool).stable_margin(STABLE_MARGIN),
            StoreWriter::new(
                Sql::from(write_pool.clone()).stable_margin(STABLE_MARGIN),
                Sql::from(cursor_pool).stable_margin(STABLE_MARGIN),
            ),
        )
            .into();
        let store = Arc::new(ExecutorDataStore::new(rw.clone())) as Arc<dyn DataStore>;
        // The consensus journal shares the single writer connection.
        let journal = Arc::new(SqlJournal::<sqlx::Sqlite>::new(write_pool)) as Arc<dyn Journal>;
        let topology = Arc::new(StaticTopology::new(id, ids.clone())) as Arc<dyn Topology>;

        // Pages load an aggregate right after writing it, possibly on another
        // node. With several nodes, a read-index barrier makes each
        // single-aggregate read wait for the writes a quorum already committed
        // (one consensus round per read). Alone, a node applies its own writes
        // before the commit returns, so the barrier would only add latency.
        //
        // A node that stays down holds every finished write on the others
        // until it returns (they compact only what every node has applied),
        // so past `absent_after` the others stop waiting for it. A node away
        // that long copies the store back from a peer on its next start.
        // `write_backlog` caps what the node keeps meanwhile. Alone, a node
        // has no one to wait for.
        let defaults = NodeConfig::default();
        let config = NodeConfig {
            linearizable_reads: has_peers,
            peer_absence_limit: match cluster {
                Some(cluster) if has_peers => cluster.absent_after(),
                _ => None,
            },
            max_commands: cluster.map_or(defaults.max_commands, |cluster| cluster.write_backlog),
            ..defaults
        };

        let mut tasks = Vec::new();
        let (sink, inbox): (
            Arc<dyn MessageSink>,
            mpsc::Receiver<evento::accord::Envelope>,
        ) = match listener {
            None => {
                let net = InMemoryNetwork::new();
                let inbox = net.register(id);
                (Arc::new(net.sink(id)), inbox)
            }
            Some(listener) => {
                let (inbox_tx, inbox_rx) = mpsc::channel(INBOX_FRAMES);
                tasks.push(serve(listener, inbox_tx));
                (Arc::new(TcpTransport::new(id, peers)), inbox_rx)
            }
        };

        let node = Node::new(
            id,
            topology,
            Arc::new(HybridLogicalClock::new(id)),
            sink,
            store,
            journal,
        )
        .with_config(config);

        if joining {
            node.recover_state_for_join()
                .await
                .context("cannot recover the journal before copying the store")?;
            node.begin_join();
            let inbox_task = node.start(inbox);
            let outcome = join_through(&node, &contacts_of(&ids, id)).await;
            if let Err(err) = outcome {
                inbox_task.abort();
                let _ = inbox_task.await;
                for task in tasks {
                    task.abort();
                }
                return Err(
                    err.context("cannot copy the store from a peer; the next start tries again")
                );
            }
            tasks.push(inbox_task);
            if let Err(err) = std::fs::remove_file(&marker) {
                tracing::warn!(marker = %marker.display(), %err, "cannot remove the copy mark");
            }
            tracing::info!("store copied from a peer");
        } else {
            node.recover_state()
                .await
                .context("cannot open the store (its journal does not match it; see `rejoin`)")?;
            tasks.push(node.start(inbox));
        }
        tasks.push(node.start_recovery());

        let copy_pending = Arc::new(AtomicBool::new(false));
        tasks.push(watch_signals(
            node.clone(),
            files.clone(),
            copy_pending.clone(),
        ));

        let mut executor = Evento::new(AccordExecutor::<Local>::new(node.clone(), rw));
        if let Some(region) = region {
            tracing::info!(region, "default routing key");
            executor = executor.default_routing_key(region);
        }
        let cluster = Arc::new(Self {
            node,
            ids,
            tasks: Mutex::new(tasks),
            joined: joining,
            copy_pending,
        });
        Ok((cluster, executor))
    }

    /// Waits, up to `limit`, until this node takes part: a node opened over
    /// a journal that holds something refuses writes until its peers
    /// confirm the journal is current. `serve` answers meanwhile with
    /// refusals; a command that writes as soon as it opens (`seed`) waits
    /// here first.
    pub async fn until_confirmed(&self, limit: Duration) -> anyhow::Result<()> {
        let started = tokio::time::Instant::now();
        while !self.node.confirmed() {
            anyhow::ensure!(
                started.elapsed() < limit,
                "this node's peers did not confirm its journal within {limit:?}: are a majority of the nodes running?"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(())
    }

    /// This node, as the pages show it.
    pub fn health(&self) -> NodeHealth {
        let absent = self.node.absent_peers();
        let peers = self
            .ids
            .iter()
            .filter(|&&id| id != self.node.id())
            .map(|&id| PeerState {
                id: id.0,
                silent_for: self.node.silent_for(id),
                given_up: absent.contains(&id),
            })
            .collect();
        NodeHealth {
            node_id: self.node.id().0,
            nodes: self.ids.len(),
            backlog: (self.node.command_count(), self.node.config().max_commands),
            peers,
            rejoin_required: self.node.rejoin_required(),
            rolled_back: self.node.rolled_back(),
            confirmed: self.node.confirmed(),
            joined_at_start: self.joined,
            copy_pending: self.copy_pending.load(Ordering::Acquire),
        }
    }

    /// A counter that moves whenever the node's applied state advances (a
    /// write applied here, or events imported from a peer). Read by the
    /// cluster tests to wait for a node to catch up.
    #[allow(dead_code)]
    pub fn activity(&self) -> u64 {
        self.node.sweep_pressure().activity
    }

    /// Warns, every `every`, when this node keeps four fifths or more of the
    /// writes it may keep for a node that is away (once, until it drops under
    /// half), and about each node this node stopped waiting for (once per
    /// node). The warnings are the operator's alert: a node at its limit
    /// refuses writes.
    pub fn watch(self: &Arc<Self>, every: Duration) -> JoinHandle<()> {
        let cluster = Arc::clone(self);
        tokio::spawn(async move {
            let mut warned_backlog = false;
            let mut warned_peers: HashSet<u64> = HashSet::new();
            loop {
                tokio::time::sleep(every).await;
                let health = cluster.health();
                let (kept, limit) = health.backlog;
                if kept * 5 >= limit * 4 {
                    if !warned_backlog {
                        warned_backlog = true;
                        tracing::warn!(
                            kept,
                            limit,
                            "this node keeps most of the writes it may for a node that is away; at the limit it refuses writes"
                        );
                    }
                } else if kept * 2 < limit {
                    warned_backlog = false;
                }
                for peer in &health.peers {
                    if peer.given_up {
                        if warned_peers.insert(peer.id) {
                            tracing::warn!(
                                node = peer.id,
                                silent_secs = peer.silent_for.as_secs(),
                                "a node has not been heard from and is no longer waited for"
                            );
                        }
                    } else {
                        warned_peers.remove(&peer.id);
                    }
                }
            }
        })
    }

    /// Stops the node: it mints nothing more, and its tasks are ended.
    pub async fn shutdown(&self) {
        self.node.retire().await;
        let tasks: Vec<JoinHandle<()>> = self
            .tasks
            .lock()
            .expect("tasks poisoned")
            .drain(..)
            .collect();
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// The other nodes, starting with the one after this node, so the nodes of a
/// cluster do not all ask the same peer first.
fn contacts_of(ids: &[NodeId], me: NodeId) -> Vec<NodeId> {
    let at = ids.iter().position(|&id| id == me).unwrap_or(0);
    ids[at + 1..]
        .iter()
        .chain(&ids[..at])
        .copied()
        .filter(|&id| id != me)
        .collect()
}

/// Joins through the contacts in turn: the peer that told this node to
/// rejoin first, when the journal remembers one, since it compacted past
/// what this node holds. A join that installed part of a snapshot stops: the
/// next contact's pages would overlap it and be skipped, leaving a gap.
async fn join_through(node: &Node, contacts: &[NodeId]) -> Result<()> {
    let mut order: Vec<NodeId> = Vec::with_capacity(contacts.len() + 1);
    if let Some(first) = node.rejoin_contact() {
        order.push(first);
    }
    for &contact in contacts {
        if !order.contains(&contact) {
            order.push(contact);
        }
    }
    let mut outcome = Err(anyhow::anyhow!("no contact to copy the store from"));
    for contact in order {
        outcome = node.join(contact).await.map(|_| ());
        match &outcome {
            Ok(()) => break,
            Err(err) if err.is::<PartialSnapshot>() => break,
            Err(err) => {
                tracing::warn!(contact = contact.0, %err, "cannot copy the store from this node");
            }
        }
    }
    outcome
}

/// Marks the store for a copy when the node is told to rejoin, or its
/// journal is found rolled back; the pages show it, writes already fail.
fn watch_signals(node: Node, files: Files, copy_pending: Arc<AtomicBool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut rejoin = node.rejoin_signal();
        let mut rolled_back = node.rolled_back_signal();
        loop {
            tokio::select! {
                changed = rejoin.changed() => {
                    if changed.is_err() { return; }
                    if !*rejoin.borrow() { continue; }
                    copy_pending.store(true, Ordering::Release);
                    match mark_for_rejoin(&files) {
                        Ok(marker) => tracing::error!(
                            marker = %marker.display(),
                            "this node fell behind the other nodes and refuses writes; restart it to copy the store from a peer"
                        ),
                        Err(err) => tracing::error!(
                            %err,
                            "this node fell behind the other nodes and refuses writes; run `rejoin` and restart it"
                        ),
                    }
                }
                changed = rolled_back.changed() => {
                    if changed.is_err() { return; }
                    if !*rolled_back.borrow() { continue; }
                    copy_pending.store(true, Ordering::Release);
                    tracing::error!(
                        "this node's journal is an older copy of itself and the node refuses writes; stop it, delete the store file, run `rejoin` and start it again"
                    );
                }
            }
        }
    })
}

/// Another node, as seen from this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerState {
    /// Its index into `cluster.peers`.
    pub id: u64,
    /// How long since it was last heard from (since this node opened, if
    /// never).
    pub silent_for: Duration,
    /// Whether this node stopped waiting for it (`cluster.absent_after_secs`).
    pub given_up: bool,
}

/// This node, for the cluster page and the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHealth {
    pub node_id: u64,
    /// Nodes in the cluster, this one included.
    pub nodes: usize,
    /// The finished writes kept for nodes that are away, and the most that
    /// may be kept before refusing new ones.
    pub backlog: (usize, usize),
    pub peers: Vec<PeerState>,
    /// The other nodes tidied up writes this node never got: it refuses
    /// writes until it copies the store back.
    pub rejoin_required: bool,
    /// The journal is an older copy of itself: the node refuses writes.
    pub rolled_back: bool,
    /// With peers, whether they confirmed the journal is current; alone,
    /// always.
    pub confirmed: bool,
    /// Whether this start copied the store from a peer.
    pub joined_at_start: bool,
    /// Whether the store is marked for a copy at the next start.
    pub copy_pending: bool,
}
