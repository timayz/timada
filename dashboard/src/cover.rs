//! The background work of a region whose node is away (#709).
//!
//! A saga follows the events of its node's region, and a node may be the
//! only one of its region: the chart gives each pod a region of its own, so
//! a node more is that much more of the work done at once. While such a node
//! is down, the payments, supplier orders, invoices and mails of what it
//! created wait for it; removed for good, they wait for ever, since a stream
//! keeps the region it was created in.
//!
//! So each node says its region on a list in the store
//! ([`timada_tenancy::aggregator::regions`]), which every node reads, and
//! looks every [`EVERY`] for the regions on it that no node heard from
//! within [`AFTER`] serves. One of the nodes up takes each such region on
//! ([`to_cover`]): it starts a copy of every saga filtered on that region's
//! key, as `server.also_serves` does from the start, with a cursor of its
//! own that goes on from where this node left that region last. It stops
//! them once a node serving the region is heard from again.
//!
//! Two nodes that disagree for a moment on who is up both take a region on,
//! or the node comes back while its work is still being done for it: the
//! sagas are written for several nodes following the same events (the nodes
//! of one region always did), so the work is done once all the same.
//!
//! The list is kept in the store, and not as what each node heard the
//! others say: a node that starts while another is away, or that copied its
//! store after it left, never heard it say its region.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use evento::subscription::SubscriptionBuilder;
use tokio::task::JoinHandle;

use crate::cluster::{self, Cluster, Executor, LiveNode, LocalReads, NodeId};
use crate::workers::{self, Worker, Workers};

/// How long a region goes without a node heard from before another takes
/// its work on. Longer than a node takes to restart for an update: the
/// first time a node takes a region on, each saga goes through that
/// region's history.
pub const AFTER: Duration = Duration::from_secs(60);

/// How often a node looks for regions to take on or hand back.
pub const EVERY: Duration = Duration::from_secs(10);

/// A saga a node runs for a region: the name the cluster page shows, and
/// its subscription filtered on the region's key.
pub type Saga = (
    String,
    Arc<dyn Fn(&str) -> SubscriptionBuilder<Executor> + Send + Sync>,
);

/// The regions of `known` whose work falls to `me`: those no node of
/// `live` (the nodes heard from, `me` included) serves, shared out among
/// those of them that have a region (a node without one takes none on, as
/// it may not serve another's besides) so each is taken on by one. None
/// while a node heard from has not said what it serves yet: its region is
/// not without a node.
pub fn to_cover(me: NodeId, live: &[LiveNode], known: &BTreeSet<String>) -> Vec<String> {
    if live.iter().any(|node| node.label.is_none()) {
        return Vec::new();
    }
    let takers: Vec<NodeId> = live
        .iter()
        .filter(|node| node.label.as_deref().is_some_and(|label| !label.is_empty()))
        .map(|node| node.id)
        .collect();
    let Some(index) = takers.iter().position(|id| *id == me) else {
        return Vec::new();
    };
    let share = timada_core::Share {
        index: index as u64,
        of: takers.len() as u64,
    };
    known
        .iter()
        .filter(|region| {
            !live.iter().any(|node| {
                node.label
                    .as_deref()
                    .is_some_and(|label| cluster::serves(label, Some(region)))
            })
        })
        .filter(|region| share.owns(region))
        .cloned()
        .collect()
}

/// What takes the regions without a node on, and hands them back.
pub struct Cover {
    cluster: Arc<Cluster>,
    /// The store through the cluster, where this node says its region.
    store: Executor,
    /// The store from this node's file: where the list is read every
    /// [`EVERY`], and what the sagas run on, as the node's own do.
    local: LocalReads,
    /// This node's region.
    region: String,
    sagas: Vec<Saga>,
    /// The node's workers, for the cluster page to show those run here.
    workers: Workers,
    after: Duration,
}

impl Cover {
    pub fn new(
        cluster: Arc<Cluster>,
        store: Executor,
        local: LocalReads,
        region: String,
        sagas: Vec<Saga>,
        workers: Workers,
        after: Duration,
    ) -> Self {
        Self {
            cluster,
            store,
            local,
            region,
            sagas,
            workers,
            after,
        }
    }

    /// Looks every `every`, until aborted. The workers it started are the
    /// node's ([`Workers`]), which stops them with the others.
    pub fn start(self, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut said = false;
            let mut covered = BTreeMap::new();
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if !said {
                    // Refused while this node reaches no majority, as at a
                    // start with the others: said at the next look.
                    match timada_tenancy::Command(self.store.clone())
                        .open_region(&self.region)
                        .await
                    {
                        Ok(()) => said = true,
                        Err(err) => tracing::warn!(
                            region = %self.region,
                            error = %format!("{err:#}"),
                            "this node's region is not on the list of regions yet"
                        ),
                    }
                }
                if let Err(err) = self.look(&mut covered).await {
                    tracing::error!(error = %format!("{err:#}"), "the regions without a node were not looked for");
                }
            }
        })
    }

    /// Takes on the regions that fall to this node and hands back the
    /// others it had.
    async fn look(&self, covered: &mut BTreeMap<String, Vec<Arc<Worker>>>) -> anyhow::Result<()> {
        let known = timada_tenancy::Command(self.local.0.clone())
            .load_regions()
            .await?;
        let wanted = to_cover(self.cluster.id(), &self.cluster.live(self.after), &known);
        let handed_back: Vec<String> = covered
            .keys()
            .filter(|region| !wanted.contains(region))
            .cloned()
            .collect();
        for region in handed_back {
            let workers = covered.remove(&region).unwrap_or_default();
            self.stop(&workers).await;
            tracing::info!(%region, "a node serves this region again, or another took it on: its background work is handed back");
        }
        for region in wanted {
            if covered.contains_key(&region) {
                continue;
            }
            match self.take_on(&region).await {
                Ok(workers) => {
                    tracing::warn!(%region, "no node heard from serves this region: this node takes its background work on");
                    covered.insert(region, workers);
                }
                Err(err) => tracing::error!(
                    %region,
                    error = %format!("{err:#}"),
                    "the background work of a region without a node was not taken on; tried again at the next look"
                ),
            }
        }
        Ok(())
    }

    /// Starts every saga on `region`'s events. All of them or none: a start
    /// that failed stops those before it.
    async fn take_on(&self, region: &str) -> anyhow::Result<Vec<Arc<Worker>>> {
        let mut started = Vec::new();
        for (name, make) in &self.sagas {
            let make = make.clone();
            let of = region.to_owned();
            let worker = workers::start(
                format!("{name} for {region}"),
                move || make(&of),
                &self.local,
            )
            .await;
            match worker {
                Ok(worker) => {
                    self.workers.add(worker.clone());
                    started.push(worker);
                }
                Err(err) => {
                    self.stop(&started).await;
                    return Err(err);
                }
            }
        }
        Ok(started)
    }

    async fn stop(&self, workers: &[Arc<Worker>]) {
        for worker in workers {
            worker.stop();
        }
        for worker in workers {
            worker.stopped().await;
            self.workers.remove(worker);
        }
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
            })
            .collect()
    }

    fn known(regions: &[&str]) -> BTreeSet<String> {
        regions.iter().map(|region| region.to_string()).collect()
    }

    /// The nodes of `live` that take `region` on.
    fn takers(live: &[LiveNode], regions: &BTreeSet<String>, region: &str) -> Vec<u64> {
        live.iter()
            .filter(|node| to_cover(node.id, live, regions).iter().any(|r| r == region))
            .map(|node| node.id.0)
            .collect()
    }

    #[test]
    fn nothing_is_taken_on_while_every_region_has_a_node() {
        let live = live(&[(0, Some("a")), (1, Some("b")), (2, Some("c"))]);
        let regions = known(&["a", "b", "c"]);
        for node in 0..3 {
            assert!(to_cover(NodeId(node), &live, &regions).is_empty());
        }
    }

    /// One node, and one only, of those left takes on the region of the
    /// node that is away; a region a node left for good is one more such.
    #[test]
    fn one_of_the_nodes_left_takes_a_region_without_a_node() {
        let live = live(&[(0, Some("a")), (2, Some("c"))]);
        let regions = known(&["a", "b", "c", "gone"]);
        assert_eq!(takers(&live, &regions, "b").len(), 1);
        assert_eq!(takers(&live, &regions, "gone").len(), 1);
        assert!(takers(&live, &regions, "a").is_empty());
        assert!(takers(&live, &regions, "c").is_empty());
    }

    /// A region another node serves besides its own (`server.also_serves`)
    /// has a node while that one is up.
    #[test]
    fn a_region_also_served_by_a_node_up_is_left_alone() {
        let live = live(&[(0, Some("a")), (2, Some("c b"))]);
        assert!(takers(&live, &known(&["a", "b", "c"]), "b").is_empty());
    }

    /// A node heard from that has not said its region yet may be the one
    /// serving it: nothing is taken on until it has.
    #[test]
    fn nothing_is_taken_on_before_every_node_heard_from_said_its_region() {
        let live = live(&[(0, Some("a")), (1, None)]);
        assert!(takers(&live, &known(&["a", "b", "c"]), "b").is_empty());
        assert!(takers(&live, &known(&["a", "b", "c"]), "c").is_empty());
    }

    /// A node without a region takes none on, and is not counted among
    /// those that do: the region is not left waiting for it.
    #[test]
    fn a_node_without_a_region_takes_none_on() {
        let live = live(&[(0, Some("")), (1, Some("b")), (2, Some("c"))]);
        let regions = known(&["a", "b", "c", "d", "e", "f"]);
        assert!(to_cover(NodeId(0), &live, &regions).is_empty());
        for region in ["a", "d", "e", "f"] {
            assert_eq!(takers(&live, &regions, region).len(), 1, "{region}");
        }
    }

    /// A node alone over a store other nodes wrote, as after a restore from
    /// one node's copy, does all of their work.
    #[test]
    fn a_node_alone_takes_every_other_region_on() {
        let live = live(&[(0, Some("a"))]);
        assert_eq!(
            to_cover(NodeId(0), &live, &known(&["a", "b", "c"])),
            ["b", "c"]
        );
    }
}
