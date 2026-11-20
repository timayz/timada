//! Whether this node should get traffic, and how far it stands behind:
//! `GET /ready`, for the balancer in front of the nodes, and the figures
//! the metrics render ([`crate::metrics`]).
//!
//! A node that serves pages from read models far behind the store (one
//! back from a night away, one that just copied the store or rebuilt a
//! read model, one whose read-model worker is stuck) shows shoppers and
//! merchants a stale shop while the balancer sees a healthy HTTP server.
//! So the answer is 200 only while the node takes writes and no read model
//! trails the store by more than `monitoring.ready_lag_secs`; the body says
//! how far each one trails, 200 or 503. A balancer polls it, an operator
//! reads it.
//!
//! Taking writes means two things: nothing ended the node's part in the
//! cluster (`NodeHealth::serving`), and it hears from most of the nodes,
//! itself counted ([`crate::cluster::reaches_majority`]). A node cut off
//! from the others refuses nothing by itself, but no write it takes can go
//! through; a rolling update that waits for a restarted node to be ready
//! must not take one that is not back among the others (#700).
//!
//! The lag of a read model is how long the oldest event its subscriptions
//! follow and have not applied yet has been waiting, asked of the store
//! each time (`Worker::lag`, evento's `SubscriptionBuilder::lag`). It is
//! found with the subscription's own filters, so an event a read model
//! does not follow never counts, however new: a store nobody writes to
//! leaves every read model at zero (#713). And no acknowledgement has to
//! happen for it to move: a worker that failed and waits to be started
//! again, one that stopped, and one whose handler does not return all read
//! as far behind as the event they stand on is old.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use topcoat::context::{Cx, context};
use topcoat::router::{
    Body, HeaderValue, RouteFuture, StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE},
    response::Response,
};

use crate::cluster::{self, Cluster, NodeHealth};
use crate::db;
use crate::store_api::StoreConnections;
use crate::workers::{WorkerState, Workers};

/// Where the pictures and keys stand is reported apart: they are this
/// node's own copies, fetched from the other nodes, and a node rebuilt on a
/// new machine gets them by `rsync` rather than through its subscription.
const PER_NODE: &[(&str, &str)] = &[("pictures", crate::media::SUBSCRIPTION)];

/// What answers `/ready` and feeds the metrics.
pub struct Readiness {
    cluster: Arc<Cluster>,
    workers: Workers,
    /// Those of them that fill the read models: what the lag is asked of.
    read_model_workers: Workers,
    connections: Arc<StoreConnections>,
    /// The most a read model may trail the store for the node to be ready;
    /// `None` reports the lag and never judges it.
    max_lag: Option<Duration>,
}

/// How this node stands, as `/ready` says it.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Whether the balancer should send traffic here.
    pub ready: bool,
    /// The node's state in a word or two, as the cluster page shows it.
    pub node: &'static str,
    /// Whether the node takes writes: nothing ended its part in the cluster
    /// and it hears from most of the nodes.
    pub serving: bool,
    /// The cluster's nodes, this one included, and how many of them this
    /// one hears from, itself counted.
    pub nodes: usize,
    pub nodes_in_touch: usize,
    /// The read model furthest behind the store, in seconds: for how long
    /// the oldest event it has not applied yet has been waiting.
    pub lag_seconds: Option<i64>,
    /// Each read model, by the name `rebuild` knows it under.
    pub read_models: Vec<Lag>,
    /// This node's own copies (the pictures), reported apart.
    pub per_node: Vec<Lag>,
    /// The subscriptions this node runs, and how many of them run.
    pub workers: usize,
    pub workers_running: usize,
}

/// How far one subscription, or the subscriptions of one read model, trail
/// the store.
#[derive(Debug, Clone, Serialize)]
pub struct Lag {
    pub name: &'static str,
    pub lag_seconds: Option<i64>,
}

/// Where one subscription stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// The key its cursor is kept under.
    pub key: String,
    /// For how long the oldest event it follows and has not acknowledged
    /// yet has been waiting, in seconds; 0 when there is none.
    pub lag: i64,
}

impl Readiness {
    /// `read_model_workers` are those of `workers` that fill the read
    /// models.
    pub fn new(
        cluster: Arc<Cluster>,
        workers: Workers,
        read_model_workers: Workers,
        connections: Arc<StoreConnections>,
        max_lag: Option<Duration>,
    ) -> Self {
        Self {
            cluster,
            workers,
            read_model_workers,
            connections,
            max_lag,
        }
    }

    /// The node's state now.
    pub async fn report(&self) -> Result<Report> {
        let health = self.cluster.health();
        let cursors: Vec<Cursor> = self
            .read_model_workers
            .lags()
            .await?
            .into_iter()
            .filter_map(|(key, lag)| {
                Some(Cursor {
                    key: key?,
                    lag: lag.waiting().as_secs() as i64,
                })
            })
            .collect();
        let read_models = db::read_models()
            .map(|read_model| Lag {
                name: read_model.name.unwrap_or_default(),
                lag_seconds: lag_of(&cursors, read_model.subscriptions),
            })
            .collect();
        let per_node = PER_NODE
            .iter()
            .map(|(name, key)| Lag {
                name,
                lag_seconds: lag_of(&cursors, &[key]),
            })
            .collect();
        let states = self.workers.states();
        let workers_running = states
            .iter()
            .filter(|(_, state)| state.is_running())
            .count();
        Ok(judge(
            &health,
            read_models,
            per_node,
            states.len(),
            workers_running,
            self.max_lag,
        ))
    }

    /// This node's consensus engine, for the metrics.
    pub fn cluster(&self) -> &Cluster {
        &self.cluster
    }

    /// Each worker's name and state, for the metrics.
    pub fn worker_states(&self) -> Vec<(String, WorkerState)> {
        self.workers.states()
    }

    /// The live store connections by tenant, for the metrics.
    pub fn connections_by_tenant(&self) -> std::collections::HashMap<String, usize> {
        self.connections.by_tenant()
    }
}

/// How far behind the store the subscriptions under `keys` are, the
/// furthest of them: a cursor's key is the subscription's own or that
/// prefixed with the node's work group (`eu.timada-catalog/read-models`).
/// `None` when no worker of this node runs one of them.
pub fn lag_of(cursors: &[Cursor], keys: &[&str]) -> Option<i64> {
    cursors
        .iter()
        .filter(|cursor| {
            keys.iter()
                .any(|key| cursor.key == *key || cursor.key.ends_with(&format!(".{key}")))
        })
        .map(|cursor| cursor.lag.max(0))
        .max()
}

/// Whether the node is ready, from what was measured.
pub fn judge(
    health: &NodeHealth,
    read_models: Vec<Lag>,
    per_node: Vec<Lag>,
    workers: usize,
    workers_running: usize,
    max_lag: Option<Duration>,
) -> Report {
    let lag_seconds = read_models.iter().filter_map(|lag| lag.lag_seconds).max();
    let within = match (max_lag, lag_seconds) {
        (Some(limit), Some(lag)) => lag <= limit.as_secs() as i64,
        _ => true,
    };
    let serving = health.serving() && cluster::reaches_majority(health);
    Report {
        ready: serving && within,
        node: node_state(health),
        serving,
        nodes: health.nodes,
        nodes_in_touch: 1 + health
            .peers
            .iter()
            .filter(|peer| cluster::in_touch(peer))
            .count(),
        lag_seconds,
        read_models,
        per_node,
        workers,
        workers_running,
    }
}

/// The node's state in a word or two: the cluster page's words.
pub fn node_state(health: &NodeHealth) -> &'static str {
    if !health.member {
        "removed from the cluster"
    } else if health.rolled_back {
        "journal rolled back"
    } else if health.founded_apart {
        "of another cluster"
    } else if health.rejoin_required || health.copy_pending {
        "copy needed"
    } else if !health.confirmed {
        "waiting for the other nodes"
    } else if !cluster::reaches_majority(health) {
        "out of touch with most nodes"
    } else {
        "serving"
    }
}

/// `GET /ready`: the report as JSON, 200 when ready and 503 otherwise, never
/// cached. On the node's own port for the balancer, and on the metrics port.
pub fn ready_route(cx: &Cx, _body: Body) -> RouteFuture<'_> {
    Box::pin(async move {
        let report = context::<Arc<Readiness>>(cx)
            .report()
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        let body = serde_json::to_string(&report)
            .map_err(|err| topcoat::Error::from_anyhow(anyhow::Error::new(err)))?;
        let mut response = Response::new(Body::from(body));
        if !report.ready {
            *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        }
        let headers = response.headers_mut();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{NodeId, PeerState};

    fn cursor(key: &str, lag: i64) -> Cursor {
        Cursor {
            key: key.into(),
            lag,
        }
    }

    fn peer(id: u64, heard: bool) -> PeerState {
        PeerState {
            id: NodeId(id),
            silent_for: Duration::from_secs(2),
            given_up: false,
            label: heard.then(String::new),
            keys: None,
        }
    }

    /// A node of three that hears from one of the two others.
    fn health(confirmed: bool, rejoin_required: bool) -> NodeHealth {
        NodeHealth {
            id: NodeId(0),
            nodes: 3,
            backlog: (0, 100_000),
            peers: vec![peer(1, true), peer(2, false)],
            confirmed,
            rejoin_required,
            rolled_back: false,
            founded_apart: false,
            copy_pending: false,
            joined_at_start: false,
            founded_at_start: false,
            member: true,
        }
    }

    /// A read model's lag is its subscriptions' furthest, the work group
    /// prefix on a cursor key notwithstanding; a read model without a
    /// cursor has none.
    #[test]
    fn a_read_models_lag_is_its_subscriptions_furthest() {
        let cursors = [
            cursor("eu.timada-tenancy/read-models", 2),
            cursor("timada-tenancy/member-audit", 7),
            cursor("timada-catalog/read-models", 1),
            cursor("other.timada-catalog/read-models-x", 99),
        ];
        let tenancy = &["timada-tenancy/read-models", "timada-tenancy/member-audit"];
        assert_eq!(lag_of(&cursors, tenancy), Some(7));
        assert_eq!(lag_of(&cursors, &["timada-catalog/read-models"]), Some(1));
        assert_eq!(lag_of(&cursors, &["timada-orders/read-models"]), None);
    }

    /// Ready takes a serving node within the limit; a node that must copy
    /// the store or trails past the limit is not, and without a limit the
    /// lag is reported only.
    #[test]
    fn ready_means_serving_and_within_the_lag_limit() {
        let lags = |lag: Option<i64>| {
            vec![Lag {
                name: "catalog",
                lag_seconds: lag,
            }]
        };
        let limit = Some(Duration::from_secs(300));
        let report = judge(
            &health(true, false),
            lags(Some(12)),
            Vec::new(),
            4,
            4,
            limit,
        );
        assert!(report.ready);
        assert_eq!(report.lag_seconds, Some(12));
        assert_eq!(report.node, "serving");
        assert!(
            !judge(
                &health(true, false),
                lags(Some(301)),
                Vec::new(),
                4,
                4,
                limit
            )
            .ready
        );
        assert!(
            judge(
                &health(true, false),
                lags(Some(301)),
                Vec::new(),
                4,
                4,
                None
            )
            .ready
        );
        assert!(judge(&health(true, false), lags(None), Vec::new(), 4, 4, limit).ready);
        let behind = judge(&health(true, true), lags(Some(0)), Vec::new(), 4, 4, limit);
        assert!(!behind.ready);
        assert_eq!(behind.node, "copy needed");
        assert!(
            !judge(
                &health(false, false),
                lags(Some(0)),
                Vec::new(),
                4,
                4,
                limit
            )
            .ready
        );
    }

    /// A node that heard from none of the others since it opened, as three
    /// nodes started over empty journals that cannot reach each other, is
    /// not ready: nothing ended its part, and no write it takes goes
    /// through (#700).
    #[test]
    fn a_node_that_reaches_no_other_node_is_not_ready() {
        let mut alone = health(true, false);
        alone.peers = vec![peer(1, false), peer(2, false)];
        let report = judge(&alone, Vec::new(), Vec::new(), 4, 4, None);
        assert!(!report.ready && !report.serving);
        assert_eq!(report.node, "out of touch with most nodes");
        assert_eq!((report.nodes, report.nodes_in_touch), (3, 1));

        let report = judge(&health(true, false), Vec::new(), Vec::new(), 4, 4, None);
        assert!(report.ready && report.serving);
        assert_eq!((report.nodes, report.nodes_in_touch), (3, 2));

        // What ended the node's part is said first.
        let mut copy = health(true, true);
        copy.peers = vec![peer(1, false), peer(2, false)];
        assert_eq!(node_state(&copy), "copy needed");
    }
}
