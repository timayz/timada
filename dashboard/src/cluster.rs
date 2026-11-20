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
//! A cluster also begins again from one node's store, copied while the
//! cluster served (`snapshot restore --from-one-node`, #597): the copy goes
//! under every node with the `found` file next to it ([`founding_mark`]),
//! and the next `serve` drops the journal that came with the copy, which is
//! its node's alone, and opens the node over the copy's events as one of a
//! new cluster. The nodes serve once most of them opened over the same
//! events. A node that did not, left out of the restore or given another
//! copy, is of another cluster than its peers: it refuses writes
//! (`NodeHealth::founded_apart`) until it is given the same copy, or copies
//! the store from them with its own journal dropped
//! (`rejoin --drop-journal`, [`mark_for_rejoin_without_journal`]).
//!
//! With peers, a read of one aggregate through the store waits behind a read
//! barrier, a consensus round with a quorum, so it reflects every write that
//! committed before it began, whichever node took the write. [`open`] also
//! hands out the same store read from this node's file as it stands,
//! [`LocalReads`], for the reads that need no such round (#466): the
//! subscriptions, whose handlers read what is already behind the event they
//! handle; the timers, which act off read-model rows derived from this file
//! and write only at the version they loaded (#533); the storefronts' reads
//! over the store API; and, read locally and asked of the cluster only when
//! that answer cannot stand ([`LocalReads::first`]), what every request asks
//! first (who it is, its tenant, its key), asked before it turns the request
//! away, and a shopper's request to pay and their order, asked while this
//! node does not hold them.

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

/// [`mark_for_rejoin`], the node's journal dropped with its store: for a
/// node of another cluster than its peers, left out when they began again
/// from one node's copy. Its journal is the old cluster's, which the new one
/// counts for nothing.
pub fn mark_for_rejoin_without_journal(store: &Path) -> Result<PathBuf> {
    RejoinMark::DiscardJournal.write(&state_dir(store))
}

/// The founding mark of the store at `store`: present from the moment the
/// store is put back as what a new cluster begins with until the node
/// opened over it. The name is evento's host's ([`mark_for_founding`]
/// returns it).
pub fn founding_mark(store: &Path) -> PathBuf {
    state_dir(store).join("found")
}

/// Asks the next open to begin a new cluster over the store at `store`: the
/// journal that came with it is dropped and its events are what the cluster
/// starts from. The same store goes under every node, each marked.
pub fn mark_for_founding(store: &Path) -> Result<PathBuf> {
    Cluster::mark_for_founding(state_dir(store))
}

/// Removes the founding mark of the store at `store`, if any.
pub fn clear_founding_mark(store: &Path) -> Result<()> {
    match std::fs::remove_file(founding_mark(store)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).context("cannot remove the founding mark"),
    }
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
/// when `cluster` is `None`. `work_group`, when set, is the default routing key
/// stamped on every new aggregate this node writes; with `also_works_for`, the
/// work groups whose work this node does, and with `also_works_for_ungrouped` it
/// does the work on what was written without one too. It tells the other
/// nodes which they are, or that it has none ([`label`]).
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
    work_group: Option<&str>,
    also_works_for: &[String],
    also_works_for_ungrouped: bool,
    store_path: &Path,
    store: Store,
) -> Result<(Arc<Cluster>, Executor, LocalReads)> {
    // The read models are derived data in the same file: dropped with the
    // store's rows, filled again by the subscriptions from the copied events.
    let read_models = store.clone();
    let mut builder = Cluster::builder(store)
        .settings(cluster.map(ClusterConfig::settings))
        .state_dir(state_dir(store_path))
        .label(label(work_group, also_works_for, also_works_for_ungrouped));
    // The work groups are routing keys set on the wrapper below, which the node
    // does not see: it is told what its subscriptions follow. Without a
    // work group it works that out for itself (the events written without a
    // key).
    if let Some(work_group) = work_group {
        builder = builder
            .serves(std::iter::once(work_group).chain(also_works_for.iter().map(String::as_str)));
        if also_works_for_ungrouped {
            builder = builder.serves_no_key();
        }
    }
    let (cluster, executor) = builder
        .before_copy(move || async move {
            crate::db::reset_store_read_models(&read_models)
                .await
                .context("cannot empty the read models before copying the store")
        })
        .open()
        .await?;
    let mut local = Evento::new(executor.local_reads());
    let mut executor = Evento::new(executor);
    if let Some(work_group) = work_group {
        tracing::info!(work_group, "default routing key");
        executor = executor.default_routing_key(work_group);
        local = local.default_routing_key(work_group);
    }
    Ok((cluster, executor, LocalReads(local)))
}

/// How long a node may go unheard before the others share out its part of
/// the work ([`timada_core::Share`]): the AliExpress reads, the newsletter
/// mails, the idle carts dropped.
pub const LIVE_WITHIN: Duration = Duration::from_secs(30);

/// How long a node may go unheard and still count as in touch: the nodes
/// talk many times a second, so one silent this long is cut off or down.
pub const IN_TOUCH_WITHIN: Duration = Duration::from_secs(10);

/// Whether this node hears from `peer`: it said something since this node
/// opened (a node says its label with what it gossips, [`label`]), and not
/// longer than [`IN_TOUCH_WITHIN`] ago.
pub fn in_touch(peer: &PeerState) -> bool {
    peer.label.is_some() && !peer.given_up && peer.silent_for < IN_TOUCH_WITHIN
}

/// Whether this node hears from enough of the others for a write to go
/// through: most of the cluster's nodes, itself counted. Alone, always. A
/// node opened over an empty journal has nothing for the others to confirm
/// (`NodeHealth::confirmed` from the start), so what it says of itself does
/// not tell whether it reaches them (#700).
pub fn reaches_majority(health: &NodeHealth) -> bool {
    let in_touch = 1 + health.peers.iter().filter(|peer| in_touch(peer)).count();
    in_touch > health.nodes / 2
}

/// This node, as the work it claims names it (`node-2`): its id in the
/// cluster, the same across restarts.
pub fn node_name(cluster: &Cluster) -> String {
    format!("node-{}", cluster.id().0)
}

/// What a node tells the other nodes about itself with its gossip: the
/// work groups whose events its sagas follow, its own first (`eu us`, one word
/// each as `Config::load` makes sure). A node without a work group follows the
/// events written without one, which a label names with the empty word: it
/// says the empty label. A node with a work group that follows them too
/// (`also_works_for_ungrouped`) ends its label with the empty word (`eu us `).
/// Every node says a label, so a node whose label has not come in yet is
/// never taken for a node without a work group.
pub fn label(
    work_group: Option<&str>,
    also_works_for: &[String],
    also_works_for_ungrouped: bool,
) -> String {
    let mut label = work_group.unwrap_or_default().to_owned();
    for served in also_works_for {
        label.push(' ');
        label.push_str(served);
    }
    if also_works_for_ungrouped && work_group.is_some() {
        label.push(' ');
    }
    label
}

/// Whether a node whose label is `label` sees the events written under
/// `work_group` (`None`: without one), and so does their work.
pub fn serves(label: &str, work_group: Option<&str>) -> bool {
    let work_group = work_group.unwrap_or_default();
    label.split(' ').any(|served| served == work_group)
}

/// Which nodes a piece of work is shared out among.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Among<'a> {
    /// Every node up: the work over the read models (the sweeps), which
    /// every node keeps whole, whatever its work group.
    All,
    /// The nodes up that see the events written under this work group (`None`:
    /// without one, which the nodes without a work group see, and those with
    /// one that also serve them): the work a saga
    /// is told about, which must fall to a node that sees its event, as the
    /// sagas follow their node's work group. A node whose label has not come in
    /// yet counts for no work group until it does.
    WorkGroup(Option<&'a str>),
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
                Among::WorkGroup(work_group) => node
                    .label
                    .as_deref()
                    .is_some_and(|label| serves(label, work_group)),
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

/// A node behind its peers, for the tests of what is read from its file.
#[cfg(test)]
pub mod lagging {
    use evento::Evento;

    use super::LocalReads;

    /// Every event of `store`, in the order they were written.
    pub async fn events(store: &evento::Turso) -> Vec<evento::Event> {
        use evento::Executor as _;
        store
            .read(
                None,
                None,
                evento::TenantFilter::All,
                evento::cursor::Args::forward(1000, None),
                None,
            )
            .await
            .unwrap()
            .edges
            .into_iter()
            .map(|edge| edge.node)
            .collect()
    }

    /// A node that holds the first `n` events of `store` alone, in a file of
    /// its own: behind it by the others.
    pub async fn behind(store: &evento::Turso, n: usize) -> LocalReads {
        use evento::Executor as _;
        let node = evento::Turso::in_memory_concurrent().await.unwrap();
        for event in events(store).await.into_iter().take(n) {
            node.write(vec![event]).await.unwrap();
        }
        LocalReads(Evento::new(node))
    }
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
                keys: None,
            })
            .collect()
    }

    fn peer(id: u64, label: Option<&str>, silent_secs: u64, given_up: bool) -> PeerState {
        PeerState {
            id: NodeId(id),
            silent_for: Duration::from_secs(silent_secs),
            given_up,
            label: label.map(str::to_owned),
            keys: None,
        }
    }

    fn health(nodes: usize, peers: Vec<PeerState>) -> NodeHealth {
        NodeHealth {
            id: NodeId(0),
            nodes,
            backlog: (0, 100_000),
            peers,
            confirmed: true,
            rejoin_required: false,
            rolled_back: false,
            founded_apart: false,
            copy_pending: false,
            joined_at_start: false,
            founded_at_start: false,
        }
    }

    /// A node of three reaches a majority with one of the two others in
    /// touch. A peer never heard from is not, however lately this node
    /// opened (its silence counts from then), nor one gone quiet or no
    /// longer waited for. A node alone reaches itself.
    #[test]
    fn a_majority_is_reached_with_most_nodes_in_touch() {
        assert!(reaches_majority(&health(1, Vec::new())));
        let quiet = || peer(2, Some(""), 10, false);
        assert!(reaches_majority(&health(
            3,
            vec![peer(1, Some("eu"), 0, false), quiet()]
        )));
        assert!(reaches_majority(&health(
            3,
            vec![peer(1, Some(""), 9, false), quiet()]
        )));
        assert!(!reaches_majority(&health(
            3,
            vec![peer(1, None, 2, false), peer(2, None, 2, false)]
        )));
        assert!(!reaches_majority(&health(
            3,
            vec![peer(1, Some(""), 10, false), quiet()]
        )));
        assert!(!reaches_majority(&health(
            3,
            vec![peer(1, Some(""), 0, true), quiet()]
        )));
        // Of five, two others are needed.
        let five = |in_touch: usize| {
            health(
                5,
                (1..5)
                    .map(|id| {
                        peer(
                            id,
                            Some(""),
                            if id as usize <= in_touch { 0 } else { 60 },
                            false,
                        )
                    })
                    .collect(),
            )
        };
        assert!(!reaches_majority(&five(1)));
        assert!(reaches_majority(&five(2)));
    }

    #[test]
    fn the_founding_mark_is_where_the_host_looks_for_it() {
        let dir = std::env::temp_dir().join(format!("timada-found-{}", ulid::Ulid::generate()));
        let store = dir.join("timada.db");
        assert_eq!(mark_for_founding(&store).unwrap(), founding_mark(&store));
        assert!(founding_mark(&store).exists());
        clear_founding_mark(&store).unwrap();
        clear_founding_mark(&store).unwrap();
        assert!(!founding_mark(&store).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_label_names_the_work_groups_a_node_serves() {
        assert_eq!(label(None, &[], false), "");
        assert_eq!(label(Some("eu"), &[], false), "eu");
        assert_eq!(
            label(Some("eu"), &["us".to_owned(), "ap".to_owned()], false),
            "eu us ap"
        );
        assert!(serves("eu us ap", Some("eu")));
        assert!(serves("eu us ap", Some("ap")));
        assert!(!serves("eu us ap", Some("a")));
        assert!(!serves("eu", Some("us")));
        // A node without a work group sees the events written without one, and
        // those alone.
        assert!(serves("", None));
        assert!(!serves("", Some("us")));
        assert!(!serves("eu us ap", None));
    }

    #[test]
    fn a_node_with_a_work_group_says_when_it_serves_the_events_without_one() {
        let keyless = label(Some("eu"), &[], true);
        assert_eq!(keyless, "eu ");
        assert!(serves(&keyless, None));
        assert!(serves(&keyless, Some("eu")));
        assert!(!serves(&keyless, Some("us")));
        let more = label(Some("eu"), &["us".to_owned()], true);
        assert_eq!(more, "eu us ");
        assert!(serves(&more, None) && serves(&more, Some("us")));
        // A node without a work group says the empty label either way.
        assert_eq!(label(None, &[], true), "");

        // Every node has a work group now, and node 2 took the work on what was
        // written before: it does all of it, and its part of eu's.
        let all = live(&[(0, Some("eu")), (1, Some("eu")), (2, Some("eu "))]);
        assert_eq!(place_among(NodeId(2), &all, Among::WorkGroup(None)), (0, 1));
        assert_eq!(
            place_among(NodeId(2), &all, Among::WorkGroup(Some("eu"))),
            (2, 3)
        );
    }

    #[test]
    fn a_work_group_s_work_is_shared_among_the_nodes_serving_it() {
        // Node 0 serves eu, node 1 us, node 2 eu and also us.
        let all = live(&[(0, Some("eu")), (1, Some("us")), (2, Some("eu us"))]);
        let (eu, us) = (Among::WorkGroup(Some("eu")), Among::WorkGroup(Some("us")));
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
    fn the_work_on_events_without_a_work_group_falls_to_the_nodes_without_one() {
        // Halfway through giving the nodes a work group: nodes 0 and 2 have none
        // yet, node 1 serves eu. The events written without a work group reach
        // nodes 0 and 2 alone, eu's node 1 alone.
        let all = live(&[(0, Some("")), (1, Some("eu")), (2, Some(""))]);
        let keyless = Among::WorkGroup(None);
        assert_eq!(place_among(NodeId(0), &all, keyless), (0, 2));
        assert_eq!(place_among(NodeId(2), &all, keyless), (1, 2));
        assert_eq!(
            place_among(NodeId(1), &all, Among::WorkGroup(Some("eu"))),
            (0, 1)
        );
        // The work over the read models is still every node's.
        assert_eq!(place_among(NodeId(2), &all, Among::All), (2, 3));
    }

    #[test]
    fn a_node_whose_label_has_not_come_in_counts_for_no_work_group() {
        // Node 1 was heard from, but not its label: nobody knows yet whether
        // it sees eu's events or those written without a work group.
        let all = live(&[(0, Some("eu")), (1, None), (2, Some(""))]);
        assert_eq!(
            place_among(NodeId(0), &all, Among::WorkGroup(Some("eu"))),
            (0, 1)
        );
        assert_eq!(place_among(NodeId(2), &all, Among::WorkGroup(None)), (0, 1));
        // It keeps the read models whole, as every node does.
        assert_eq!(place_among(NodeId(2), &all, Among::All), (2, 3));
    }

    #[test]
    fn a_node_counts_itself_whatever_it_saw() {
        // Node 0 is asked about a us event it does not serve (kept from
        // before its work group changed, say): it is still placed, among the us
        // nodes. Node 1 does not count it and takes every us piece: node 0's
        // part is taken twice, which the work bears, and none is dropped.
        let all = live(&[(0, Some("eu")), (1, Some("us")), (2, Some(""))]);
        let us = Among::WorkGroup(Some("us"));
        assert_eq!(place_among(NodeId(0), &all, us), (0, 2));
        assert_eq!(place_among(NodeId(1), &all, us), (0, 1));
        // A node alone, or one not yet in its own list, owns everything.
        assert_eq!(
            place_among(NodeId(0), &[], Among::WorkGroup(Some("eu"))),
            (0, 1)
        );
        assert_eq!(
            place_among(NodeId(0), &live(&[(0, None)]), Among::All),
            (0, 1)
        );
    }
}
