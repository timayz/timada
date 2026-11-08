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
use std::time::Duration;

use anyhow::{Context, Result};
use evento::Evento;
use evento::cluster::host::RejoinMark;
// The test harnesses include this module and use only `open`.
#[allow(unused_imports)]
pub use evento::cluster::{Cluster, LiveNode, NodeHealth, NodeId, PeerState};

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
/// stamped on every new aggregate this node writes; with `also_serves`, the
/// regions whose work this node does, which it tells the other nodes
/// ([`label`]).
///
/// When the store is marked for a copy (see [`mark_for_rejoin`]) and the
/// node has peers, the store is emptied (the journal is kept), the read
/// models are emptied, and the node opens by joining: a peer's store is
/// installed page by page before anything is served. The mark stays when
/// the copy fails, so the next start tries again.
pub async fn open(
    cluster: Option<&ClusterConfig>,
    region: Option<&str>,
    also_serves: &[String],
    store_path: &Path,
    store: Store,
) -> Result<(Arc<Cluster>, Executor)> {
    // The read models are derived data in the same file: emptied with the
    // store, filled again by the subscriptions from the copied events.
    let read_models = store.clone();
    let mut builder = Cluster::builder(store)
        .settings(cluster.map(ClusterConfig::settings))
        .state_dir(state_dir(store_path))
        .before_copy(move || async move {
            crate::db::reset_store_read_models(&read_models)
                .await
                .context("cannot empty the read models before copying the store")
        });
    if let Some(label) = label(region, also_serves) {
        builder = builder.label(label);
    }
    let (cluster, executor) = builder.open().await?;
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

/// What a node tells the other nodes about itself with its gossip: the
/// regions whose work it does, its own first (`eu us`, one word each as
/// `Config::load` makes sure). Nothing without a region: such a node does
/// every region's work, and says so by saying nothing.
pub fn label(region: Option<&str>, also_serves: &[String]) -> Option<String> {
    let region = region?;
    let mut label = region.to_owned();
    for served in also_serves {
        label.push(' ');
        label.push_str(served);
    }
    Some(label)
}

/// Whether a node whose label is `label` does `region`'s work: a node
/// without a region (no label) does every region's.
pub fn serves(label: Option<&str>, region: &str) -> bool {
    match label {
        Some(label) => label.split(' ').any(|served| served == region),
        None => true,
    }
}

/// `me`'s place among the nodes of `live` doing `region`'s work, and how
/// many they are: `(index, of)`. Every node without a region counts; `me`
/// always does, so it owns its part of what it alone saw. No region: every
/// live node, for the work every node does (the sweeps over the read models).
pub fn place_among(me: NodeId, live: &[LiveNode], region: Option<&str>) -> (u64, u64) {
    let mut index = 0;
    let mut of = 0;
    for node in live {
        let counts =
            node.id == me || region.is_none_or(|region| serves(node.label.as_deref(), region));
        if !counts {
            continue;
        }
        if node.id == me {
            index = of;
        }
        of += 1;
    }
    (index, of.max(1))
}

/// This node's place among the nodes heard from within `within` that do
/// `region`'s work ([`place_among`]): what the AliExpress reads of a
/// region's items are shared out by, so a `us` item's read falls to a node
/// that sees `us` events (evento's sagas follow the node's region).
pub fn live_place(cluster: &Cluster, within: Duration, region: Option<&str>) -> (u64, u64) {
    place_among(cluster.id(), &cluster.live(within), region)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(nodes: &[(u64, Option<&str>)]) -> Vec<LiveNode> {
        nodes
            .iter()
            .map(|(id, label)| LiveNode {
                id: NodeId(*id),
                label: label.map(str::to_owned),
            })
            .collect()
    }

    #[test]
    fn a_label_names_the_regions_a_node_serves() {
        assert_eq!(label(None, &[]), None);
        assert_eq!(label(Some("eu"), &[]).as_deref(), Some("eu"));
        assert_eq!(
            label(Some("eu"), &["us".to_owned(), "ap".to_owned()]).as_deref(),
            Some("eu us ap")
        );
        assert!(serves(Some("eu us ap"), "eu"));
        assert!(serves(Some("eu us ap"), "ap"));
        assert!(!serves(Some("eu us ap"), "a"));
        assert!(!serves(Some("eu"), "us"));
        assert!(
            serves(None, "us"),
            "a node without a region does every region's work"
        );
    }

    #[test]
    fn a_region_s_work_is_shared_among_the_nodes_serving_it() {
        // Node 0 serves eu, node 1 us, node 2 eu and also us.
        let all = live(&[(0, Some("eu")), (1, Some("us")), (2, Some("eu us"))]);
        assert_eq!(place_among(NodeId(0), &all, Some("eu")), (0, 2));
        assert_eq!(place_among(NodeId(2), &all, Some("eu")), (1, 2));
        assert_eq!(place_among(NodeId(1), &all, Some("us")), (0, 2));
        assert_eq!(place_among(NodeId(2), &all, Some("us")), (1, 2));
        // Without a region, every node: the sweeps over every region's rows.
        assert_eq!(place_among(NodeId(1), &all, None), (1, 3));
        // Node 1 is down: node 2 does us's work alone.
        let without_1 = live(&[(0, Some("eu")), (2, Some("eu us"))]);
        assert_eq!(place_among(NodeId(2), &without_1, Some("us")), (0, 1));
        assert_eq!(place_among(NodeId(0), &without_1, Some("eu")), (0, 2));
    }

    #[test]
    fn a_node_counts_itself_whatever_it_saw() {
        // Node 0 saw a us event it does not serve (a node without a region
        // sees every region's): it is still placed, among the us nodes.
        let all = live(&[(0, Some("eu")), (1, Some("us")), (2, None)]);
        assert_eq!(place_among(NodeId(0), &all, Some("us")), (0, 3));
        assert_eq!(place_among(NodeId(2), &all, Some("us")), (1, 2));
        // A node alone, or one not yet in its own list, owns everything.
        assert_eq!(place_among(NodeId(0), &[], Some("eu")), (0, 1));
        assert_eq!(place_among(NodeId(0), &live(&[(0, None)]), None), (0, 1));
    }
}
