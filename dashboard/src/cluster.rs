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
//! empties the store, keeps the journal, drops and recreates the read models
//! (`db::reset_store_read_models`, the host's `before_copy` hook) and opens
//! the node by joining, which copies a peer's store page by page. A journal
//! found to be an older copy of itself is marked the same way, journal
//! included. The `rejoin` command sets the mark by hand, for a store that
//! was lost.
//!
//! With peers, a read of one aggregate through the store waits behind a read
//! barrier, a consensus round with a quorum, so it reflects every write that
//! committed before it began, whichever node took the write. [`open`] also
//! hands out the same store read from this node's file as it stands,
//! [`LocalReads`], for the reads that need no such round (#466): the
//! subscriptions, whose handlers read what is already behind the event they
//! handle; the storefronts' reads over the store API; and what every request
//! asks first (who it is, its tenant, its key), read locally and asked of the
//! cluster only before it turns the request away ([`LocalReads::first`]).

use std::future::Future;
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

/// The store read from this node's own file as it stands: a read of one
/// aggregate waits for no consensus round, so it may miss a write another
/// node committed a moment ago (evento's `ClusterExecutor::local_reads`).
/// Writes go through the cluster as they do through [`Executor`]. A node
/// reads its own writes either way: a write returns once this node applied
/// it.
#[derive(Clone)]
pub struct LocalReads(pub Executor);

impl std::ops::Deref for LocalReads {
    type Target = Executor;

    fn deref(&self) -> &Executor {
        &self.0
    }
}

impl LocalReads {
    /// Reads with this node's file first, and keeps that answer when
    /// `settled` says it may stand; otherwise reads again through `store`,
    /// the cluster. For what turns a request away when missing (a session, a
    /// key, a tenant that lists the member): what another node wrote a
    /// moment ago, a sign-in whose redirect landed here, is waited for
    /// before the request is refused.
    pub async fn first<T, F, Fut>(
        &self,
        store: &Executor,
        read: F,
        settled: impl FnOnce(&T) -> bool,
    ) -> T
    where
        F: Fn(Executor) -> Fut,
        Fut: Future<Output = T>,
    {
        let answer = read(self.0.clone()).await;
        if settled(&answer) {
            return answer;
        }
        read(store.clone()).await
    }
}

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

/// The name of the mark the `rebuild` command leaves next to the store: the
/// next `serve` catches the read models up before it listens, as it does
/// after a copy of the store, so a rebuilt read model is never served empty.
const CATCH_UP_MARK: &str = "catch-up";

/// The catch-up mark of the store at `store`.
pub fn catch_up_mark(store: &Path) -> PathBuf {
    state_dir(store).join(CATCH_UP_MARK)
}

/// Asks the next `serve` over the store at `store` to catch the read models
/// up before it listens.
pub fn mark_for_catch_up(store: &Path) -> Result<PathBuf> {
    let mark = catch_up_mark(store);
    std::fs::write(&mark, "the read models catch up before the node listens\n")
        .with_context(|| format!("cannot write {}", mark.display()))?;
    Ok(mark)
}

/// Removes the catch-up mark of the store at `store`, if any.
pub fn clear_catch_up_mark(store: &Path) -> Result<()> {
    match std::fs::remove_file(catch_up_mark(store)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).context("cannot remove the catch-up mark"),
    }
}

/// Opens the node over the store at `store_path` and starts it. Single node
/// when `cluster` is `None`. `region`, when set, is the default routing key
/// stamped on every new aggregate this node writes; with `also_serves`, the
/// regions whose work this node does. It tells the other nodes which they
/// are, or that it has none ([`label`]).
///
/// When the store is marked for a copy (see [`mark_for_rejoin`]) and the
/// node has peers, the store is emptied (the journal is kept), the read
/// models are dropped and recreated, and the node opens by joining: a
/// peer's store is installed page by page before anything is served. The
/// mark stays when the copy fails, so the next start tries again.
///
/// The store comes back twice over the same node: as the node reads it
/// (behind a read barrier with peers) and as [`LocalReads`].
pub async fn open(
    cluster: Option<&ClusterConfig>,
    region: Option<&str>,
    also_serves: &[String],
    store_path: &Path,
    store: Store,
) -> Result<(Arc<Cluster>, Executor, LocalReads)> {
    // The read models are derived data in the same file: dropped with the
    // store's rows, filled again by the subscriptions from the copied events.
    let read_models = store.clone();
    let (cluster, executor) = Cluster::builder(store)
        .settings(cluster.map(ClusterConfig::settings))
        .state_dir(state_dir(store_path))
        .label(label(region, also_serves))
        .before_copy(move || async move {
            crate::db::reset_store_read_models(&read_models)
                .await
                .context("cannot empty the read models before copying the store")
        })
        .open()
        .await?;
    let mut local = Evento::new(executor.local_reads());
    let mut executor = Evento::new(executor);
    if let Some(region) = region {
        tracing::info!(region, "default routing key");
        executor = executor.default_routing_key(region);
        local = local.default_routing_key(region);
    }
    Ok((cluster, executor, LocalReads(local)))
}

/// How long a node may go unheard before the others share out its part of
/// the work ([`timada_core::Share`]): the AliExpress reads, the newsletter
/// mails, the idle carts dropped.
pub const LIVE_WITHIN: Duration = Duration::from_secs(30);

/// This node, as the work it claims names it (`node-2`): its id in the
/// cluster, the same across restarts.
pub fn node_name(cluster: &Cluster) -> String {
    format!("node-{}", cluster.id().0)
}

/// What a node tells the other nodes about itself with its gossip: the
/// regions whose events its sagas follow, its own first (`eu us`, one word
/// each as `Config::load` makes sure). A node without a region follows the
/// events written without one, which a label names with the empty word: it
/// says the empty label. Every node says one, so a node whose label has not
/// come in yet is never taken for a node without a region.
pub fn label(region: Option<&str>, also_serves: &[String]) -> String {
    let mut label = region.unwrap_or_default().to_owned();
    for served in also_serves {
        label.push(' ');
        label.push_str(served);
    }
    label
}

/// Whether a node whose label is `label` sees the events written under
/// `region` (`None`: without one), and so does their work.
pub fn serves(label: &str, region: Option<&str>) -> bool {
    let region = region.unwrap_or_default();
    label.split(' ').any(|served| served == region)
}

/// Which nodes a piece of work is shared out among.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Among<'a> {
    /// Every node up: the work over the read models (the sweeps), which
    /// every node keeps whole, whatever its region.
    All,
    /// The nodes up that see the events written under this region (`None`:
    /// without one, which the nodes without a region see): the work a saga
    /// is told about, which must fall to a node that sees its event, as the
    /// sagas follow their node's region. A node whose label has not come in
    /// yet counts for no region until it does.
    Region(Option<&'a str>),
}

/// `me`'s place among the nodes of `live` that `among` names, and how many
/// they are: `(index, of)`. `me` always counts, so it owns its part of what
/// it alone saw.
pub fn place_among(me: NodeId, live: &[LiveNode], among: Among<'_>) -> (u64, u64) {
    let mut index = 0;
    let mut of = 0;
    for node in live {
        let counts = node.id == me
            || match among {
                Among::All => true,
                Among::Region(region) => node
                    .label
                    .as_deref()
                    .is_some_and(|label| serves(label, region)),
            };
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

/// This node's place among the nodes heard from within `within` that
/// `among` names ([`place_among`]).
pub fn live_place(cluster: &Cluster, within: Duration, among: Among<'_>) -> (u64, u64) {
    place_among(cluster.id(), &cluster.live(within), among)
}

/// Which of the nodes up this one is, at the moment of asking, among those
/// the work is shared out among ([`live_place`] within [`LIVE_WITHIN`]).
pub type ShareOf = Arc<dyn Fn(Among<'_>) -> timada_core::Share + Send + Sync>;

/// [`ShareOf`] for the nodes `cluster` hears from.
pub fn share_of(cluster: &Arc<Cluster>) -> ShareOf {
    let cluster = cluster.clone();
    Arc::new(move |among: Among<'_>| {
        let (index, of) = live_place(&cluster, LIVE_WITHIN, among);
        timada_core::Share { index, of }
    })
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
        assert_eq!(label(None, &[]), "");
        assert_eq!(label(Some("eu"), &[]), "eu");
        assert_eq!(
            label(Some("eu"), &["us".to_owned(), "ap".to_owned()]),
            "eu us ap"
        );
        assert!(serves("eu us ap", Some("eu")));
        assert!(serves("eu us ap", Some("ap")));
        assert!(!serves("eu us ap", Some("a")));
        assert!(!serves("eu", Some("us")));
        // A node without a region sees the events written without one, and
        // those alone.
        assert!(serves("", None));
        assert!(!serves("", Some("us")));
        assert!(!serves("eu us ap", None));
    }

    #[test]
    fn a_region_s_work_is_shared_among_the_nodes_serving_it() {
        // Node 0 serves eu, node 1 us, node 2 eu and also us.
        let all = live(&[(0, Some("eu")), (1, Some("us")), (2, Some("eu us"))]);
        let (eu, us) = (Among::Region(Some("eu")), Among::Region(Some("us")));
        assert_eq!(place_among(NodeId(0), &all, eu), (0, 2));
        assert_eq!(place_among(NodeId(2), &all, eu), (1, 2));
        assert_eq!(place_among(NodeId(1), &all, us), (0, 2));
        assert_eq!(place_among(NodeId(2), &all, us), (1, 2));
        // Every node, for the work over the read models.
        assert_eq!(place_among(NodeId(1), &all, Among::All), (1, 3));
        // Node 1 is down: node 2 does us's work alone.
        let without_1 = live(&[(0, Some("eu")), (2, Some("eu us"))]);
        assert_eq!(place_among(NodeId(2), &without_1, us), (0, 1));
        assert_eq!(place_among(NodeId(0), &without_1, eu), (0, 2));
    }

    #[test]
    fn the_work_on_events_without_a_region_falls_to_the_nodes_without_one() {
        // Halfway through giving the nodes a region: nodes 0 and 2 have none
        // yet, node 1 serves eu. The events written without a region reach
        // nodes 0 and 2 alone, eu's node 1 alone.
        let all = live(&[(0, Some("")), (1, Some("eu")), (2, Some(""))]);
        let keyless = Among::Region(None);
        assert_eq!(place_among(NodeId(0), &all, keyless), (0, 2));
        assert_eq!(place_among(NodeId(2), &all, keyless), (1, 2));
        assert_eq!(
            place_among(NodeId(1), &all, Among::Region(Some("eu"))),
            (0, 1)
        );
        // The work over the read models is still every node's.
        assert_eq!(place_among(NodeId(2), &all, Among::All), (2, 3));
    }

    #[test]
    fn a_node_whose_label_has_not_come_in_counts_for_no_region() {
        // Node 1 was heard from, but not its label: nobody knows yet whether
        // it sees eu's events or those written without a region.
        let all = live(&[(0, Some("eu")), (1, None), (2, Some(""))]);
        assert_eq!(
            place_among(NodeId(0), &all, Among::Region(Some("eu"))),
            (0, 1)
        );
        assert_eq!(place_among(NodeId(2), &all, Among::Region(None)), (0, 1));
        // It keeps the read models whole, as every node does.
        assert_eq!(place_among(NodeId(2), &all, Among::All), (2, 3));
    }

    #[test]
    fn a_node_counts_itself_whatever_it_saw() {
        // Node 0 is asked about a us event it does not serve (kept from
        // before its region changed, say): it is still placed, among the us
        // nodes. Node 1 does not count it and takes every us piece: node 0's
        // part is taken twice, which the work bears, and none is dropped.
        let all = live(&[(0, Some("eu")), (1, Some("us")), (2, Some(""))]);
        let us = Among::Region(Some("us"));
        assert_eq!(place_among(NodeId(0), &all, us), (0, 2));
        assert_eq!(place_among(NodeId(1), &all, us), (0, 1));
        // A node alone, or one not yet in its own list, owns everything.
        assert_eq!(
            place_among(NodeId(0), &[], Among::Region(Some("eu"))),
            (0, 1)
        );
        assert_eq!(
            place_among(NodeId(0), &live(&[(0, None)]), Among::All),
            (0, 1)
        );
    }
}
