//! The evento executor: Accord consensus over the local SQLite store.
//!
//! Writes are coordinated through the cluster (strictly serializable,
//! leaderless; a quorum of `N = 2f+1` nodes must agree). Reads, snapshots and
//! subscriptions are served from this node's own database. Without a
//! `[cluster]` section the node talks to itself over the in-memory transport,
//! so the same executor type serves both deployments.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use evento_accord::{
    AccordExecutor, DataStore, ExecutorDataStore, HybridLogicalClock, InMemoryNetwork, Journal,
    MessageSink, Node, NodeId, StaticTopology, TcpTransport, Topology,
};
use evento_sql::{Sql, SqlJournal};
use sqlx::SqlitePool;
use tokio::sync::mpsc;

use crate::config::ClusterConfig;

pub type Executor = AccordExecutor<Sql<sqlx::Sqlite>>;

/// Builds the executor over `pool`. Single node when `cluster` is `None`.
pub async fn build(cluster: Option<&ClusterConfig>, pool: SqlitePool) -> Result<Executor> {
    // One `Sql` instance for both the Accord data store and the executor: two
    // `.into()` from the same pool would not share the write-wake channel that
    // subscriptions rely on.
    let sql: Sql<sqlx::Sqlite> = pool.clone().into();
    let store = Arc::new(ExecutorDataStore::new(sql.clone())) as Arc<dyn DataStore>;
    let journal = Arc::new(SqlJournal::<sqlx::Sqlite>::new(pool)) as Arc<dyn Journal>;

    let node = match cluster {
        None => single_node(store, journal),
        Some(cluster) => cluster_node(cluster, store, journal).await?,
    };
    node.start_recovery();

    Ok(AccordExecutor::new(node, sql))
}

fn single_node(store: Arc<dyn DataStore>, journal: Arc<dyn Journal>) -> Node {
    let id = NodeId(0);
    let network = InMemoryNetwork::new();
    let inbox = network.register(id);

    let node = Node::new(
        id,
        Arc::new(StaticTopology::new(id, vec![id])) as Arc<dyn Topology>,
        Arc::new(HybridLogicalClock::new(id)),
        Arc::new(network.sink(id)) as Arc<dyn MessageSink>,
        store,
        journal,
    );
    node.start(inbox);

    tracing::info!("evento: single node");
    node
}

async fn cluster_node(
    cluster: &ClusterConfig,
    store: Arc<dyn DataStore>,
    journal: Arc<dyn Journal>,
) -> Result<Node> {
    let id = NodeId(cluster.node_id);
    let ids: Vec<NodeId> = (0..cluster.peers.len() as u64).map(NodeId).collect();
    let peers: HashMap<NodeId, SocketAddr> = ids
        .iter()
        .copied()
        .zip(cluster.peers.iter().copied())
        .collect();
    let listen = cluster.listen_addr();

    // Inbound: accept peer connections and feed this node's inbox.
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot bind accord listener on {listen}"))?;
    let (inbox_tx, inbox_rx) = mpsc::channel(1024);
    evento_accord::serve(listener, inbox_tx);

    let node = Node::new(
        id,
        Arc::new(StaticTopology::new(id, ids)) as Arc<dyn Topology>,
        Arc::new(HybridLogicalClock::new(id)),
        // Outbound: lazily connects to each peer over TCP.
        Arc::new(TcpTransport::new(id, peers)) as Arc<dyn MessageSink>,
        store,
        journal,
    );
    node.start(inbox_rx);

    tracing::info!(
        node = cluster.node_id,
        %listen,
        size = cluster.peers.len(),
        "evento: accord cluster node"
    );
    Ok(node)
}
