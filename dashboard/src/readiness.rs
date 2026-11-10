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
//! The lag of a read model is what its subscription recorded with its
//! cursor the last time it acknowledged (how far the event it reached was
//! behind the store's newest), raised to the age of the store's newest
//! event when that event came after the last acknowledgement: a worker
//! that stopped acknowledging reads as far behind as the store moved on,
//! not as the small figure it last wrote.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use topcoat::context::{Cx, app_context};
use topcoat::router::{
    Body, HeaderValue, RouteFuture, StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE},
    response::Response,
};

use crate::cluster::{Cluster, NodeHealth};
use crate::db::{self, Db};
use crate::store_api::StoreConnections;
use crate::workers::{WorkerState, Workers};

/// Where the pictures and keys stand is reported apart: they are this
/// node's own copies, fetched from the other nodes, and a node rebuilt on a
/// new machine gets them by `rsync` rather than through its subscription.
const PER_NODE: &[(&str, &str)] = &[("pictures", crate::media::SUBSCRIPTION)];

/// What answers `/ready` and feeds the metrics.
pub struct Readiness {
    db: Db,
    cluster: Arc<Cluster>,
    workers: Workers,
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
    /// Whether the node takes writes.
    pub serving: bool,
    /// The read model furthest behind the store, in seconds; `null` until
    /// the subscriptions first ran.
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

/// A subscription's cursor row, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub key: String,
    /// What the subscription recorded at its last acknowledgement.
    pub lag: i64,
    /// When it last acknowledged (unix seconds); `None` until it did.
    pub updated_at: Option<i64>,
}

impl Readiness {
    pub fn new(
        db: Db,
        cluster: Arc<Cluster>,
        workers: Workers,
        connections: Arc<StoreConnections>,
        max_lag: Option<Duration>,
    ) -> Self {
        Self {
            db,
            cluster,
            workers,
            connections,
            max_lag,
        }
    }

    /// The node's state now.
    pub async fn report(&self) -> Result<Report> {
        let health = self.cluster.health();
        let cursors: Vec<(String, i64, Option<i64>)> = self
            .db
            .fetch_all("SELECT key, lag, updated_at FROM subscriber", ())
            .await?;
        let cursors: Vec<Cursor> = cursors
            .into_iter()
            .map(|(key, lag, updated_at)| Cursor {
                key,
                lag,
                updated_at,
            })
            .collect();
        let newest: Option<i64> = self
            .db
            .fetch_scalar("SELECT max(timestamp) FROM event", ())
            .await?;
        let now = crate::auth::now() as i64;
        let read_models = db::read_models()
            .map(|read_model| Lag {
                name: read_model.name.unwrap_or_default(),
                lag_seconds: lag_of(&cursors, read_model.subscriptions, newest, now),
            })
            .collect();
        let per_node = PER_NODE
            .iter()
            .map(|(name, key)| Lag {
                name,
                lag_seconds: lag_of(&cursors, &[key], newest, now),
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
/// prefixed with the node's region (`eu.timada-catalog/read-models`).
/// `None` when none of them has a cursor yet.
pub fn lag_of(cursors: &[Cursor], keys: &[&str], newest: Option<i64>, now: i64) -> Option<i64> {
    cursors
        .iter()
        .filter(|cursor| {
            keys.iter()
                .any(|key| cursor.key == *key || cursor.key.ends_with(&format!(".{key}")))
        })
        .map(|cursor| effective_lag(cursor, newest, now))
        .max()
}

/// The lag a cursor stands for now: what it recorded, or the age of the
/// store's newest event when that came after the cursor's last
/// acknowledgement, whichever is more.
pub fn effective_lag(cursor: &Cursor, newest: Option<i64>, now: i64) -> i64 {
    let behind_since = newest.filter(|newest| cursor.updated_at.is_none_or(|at| at < *newest));
    match behind_since {
        Some(newest) => cursor.lag.max(now.saturating_sub(newest)),
        None => cursor.lag,
    }
    .max(0)
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
    let serving = health.serving();
    Report {
        ready: serving && within,
        node: node_state(health),
        serving,
        lag_seconds,
        read_models,
        per_node,
        workers,
        workers_running,
    }
}

/// The node's state in a word or two: the cluster page's words.
pub fn node_state(health: &NodeHealth) -> &'static str {
    if health.rolled_back {
        "journal rolled back"
    } else if health.rejoin_required || health.copy_pending {
        "copy needed"
    } else if !health.confirmed {
        "waiting for the other nodes"
    } else {
        "serving"
    }
}

/// `GET /ready`: the report as JSON, 200 when ready and 503 otherwise, never
/// cached. On the node's own port for the balancer, and on the metrics port.
pub fn ready_route(cx: &Cx, _body: Body) -> RouteFuture<'_> {
    Box::pin(async move {
        let report = app_context::<Arc<Readiness>>(cx)
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
    use crate::cluster::NodeId;

    fn cursor(key: &str, lag: i64, updated_at: Option<i64>) -> Cursor {
        Cursor {
            key: key.into(),
            lag,
            updated_at,
        }
    }

    fn health(confirmed: bool, rejoin_required: bool) -> NodeHealth {
        NodeHealth {
            id: NodeId(0),
            nodes: 3,
            backlog: (0, 100_000),
            peers: Vec::new(),
            confirmed,
            rejoin_required,
            rolled_back: false,
            copy_pending: false,
            joined_at_start: false,
        }
    }

    /// A cursor acknowledged after the store's newest event stands where it
    /// recorded; one that stopped acknowledging before the store moved on
    /// stands as far behind as the store did.
    #[test]
    fn a_stalled_cursor_reads_as_far_behind_as_the_store_moved() {
        let now = 1_000;
        assert_eq!(effective_lag(&cursor("k", 3, Some(990)), Some(980), now), 3);
        assert_eq!(
            effective_lag(&cursor("k", 3, Some(900)), Some(950), now),
            50
        );
        assert_eq!(effective_lag(&cursor("k", 3, None), Some(950), now), 50);
        assert_eq!(effective_lag(&cursor("k", 3, None), None, now), 3);
        assert_eq!(
            effective_lag(&cursor("k", 0, Some(1_100)), Some(1_200), 1_000),
            0
        );
    }

    /// A read model's lag is its subscriptions' furthest, the region
    /// prefix on a cursor key notwithstanding; a read model without a
    /// cursor has none.
    #[test]
    fn a_read_models_lag_is_its_subscriptions_furthest() {
        let cursors = [
            cursor("eu.timada-tenancy/read-models", 2, Some(1_000)),
            cursor("timada-tenancy/member-audit", 7, Some(1_000)),
            cursor("timada-catalog/read-models", 1, Some(1_000)),
            cursor("other.timada-catalog/read-models-x", 99, Some(1_000)),
        ];
        let tenancy = &["timada-tenancy/read-models", "timada-tenancy/member-audit"];
        assert_eq!(lag_of(&cursors, tenancy, Some(900), 1_000), Some(7));
        assert_eq!(
            lag_of(&cursors, &["timada-catalog/read-models"], Some(900), 1_000),
            Some(1)
        );
        assert_eq!(
            lag_of(&cursors, &["timada-orders/read-models"], Some(900), 1_000),
            None
        );
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
}
