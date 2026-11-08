//! The consensus node: the one node of the cluster this process runs, over
//! the store, opened from the `[cluster]` section of the configuration
//! through evento's host (`evento::cluster::Cluster`).
//!
//! The store (every tenant's events, the snapshots, the subscription cursors
//! and the consensus journal) is the one file of `database.path`, opened for
//! several writers (`db::open_store`): the node and the journal each write on
//! a connection of their own, the read models in the file on theirs. Writes
//! are coordinated through the cluster (strictly serializable, leaderless; a
//! quorum of `N = 2f+1` nodes must agree); reads, snapshots and subscriptions
//! are served from this node's own file. Without a `[cluster]` section the
//! node talks to itself in memory, so the same code serves both deployments.
//!
//! A node that was away longer than `cluster.absent_after_secs` while writes
//! went through is told so when it returns: its peers tidied those writes up
//! without it and cannot send them any more. The host then refuses writes
//! (`writes_paused` on the pages) and marks the store for a copy (the
//! `rejoin` file next to the store, [`rejoin_mark`]); the next `serve`
//! empties the store, keeps the journal, empties the read models
//! (`db::reset_store_read_models`, the host's `before_copy` hook) and opens
//! the node by joining, which copies a peer's store page by page. A journal
//! found to be an older copy of itself is marked the same way, journal
//! included. The `rejoin` command sets the mark by hand, for a store that
//! was lost.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use evento::Evento;
use evento::cluster::host::RejoinMark;
// The test harnesses include this module and use only `open`.
#[allow(unused_imports)]
pub use evento::cluster::{Cluster, NodeHealth, PeerState};

use crate::config::ClusterConfig;
use crate::db::Store;

/// The store: the cluster over this node's file (see [`Store`]). Type-erased
/// by [`Evento`], which also carries the node's default routing key.
pub type Executor = Evento;

/// Where the host keeps what it knows about the store at `store`: the
/// directory the store's file is in.
pub fn state_dir(store: &Path) -> PathBuf {
    match store.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The rejoin mark of the store at `store`: present from the moment a copy
/// is asked for until one finished.
pub fn rejoin_mark(store: &Path) -> PathBuf {
    RejoinMark::path(&state_dir(store))
}

/// Asks the next open to copy the store at `store` from a peer: the mark is
/// written, nothing else happens until `serve` starts.
pub fn mark_for_rejoin(store: &Path) -> Result<PathBuf> {
    Cluster::mark_for_rejoin(state_dir(store))
}

/// Removes the mark of the store at `store`, if any.
pub fn clear_rejoin_mark(store: &Path) -> Result<()> {
    RejoinMark::clear(&state_dir(store))
}

/// Opens the node over the store at `store_path` and starts it. Single node
/// when `cluster` is `None`. `region`, when set, is the default routing key
/// stamped on every new aggregate this node writes.
///
/// When the store is marked for a copy (see [`mark_for_rejoin`]) and the
/// node has peers, the store is emptied (the journal is kept), the read
/// models are emptied, and the node opens by joining: a peer's store is
/// installed page by page before anything is served. The mark stays when
/// the copy fails, so the next start tries again.
pub async fn open(
    cluster: Option<&ClusterConfig>,
    region: Option<&str>,
    store_path: &Path,
    store: Store,
) -> Result<(Arc<Cluster>, Executor)> {
    // The read models are derived data in the same file: emptied with the
    // store, filled again by the subscriptions from the copied events.
    let read_models = store.clone();
    let (cluster, executor) = Cluster::builder(store)
        .settings(cluster.map(ClusterConfig::settings))
        .state_dir(state_dir(store_path))
        .before_copy(move || async move {
            crate::db::reset_store_read_models(&read_models)
                .await
                .context("cannot empty the read models before copying the store")
        })
        .open()
        .await?;
    let mut executor = Evento::new(executor);
    if let Some(region) = region {
        tracing::info!(region, "default routing key");
        executor = executor.default_routing_key(region);
    }
    Ok((cluster, executor))
}

/// This node, as the work it claims names it (`node-2`): its id in the
/// cluster, the same across restarts.
pub fn node_name(cluster: &Cluster) -> String {
    format!("node-{}", cluster.id().0)
}
