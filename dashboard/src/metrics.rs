//! The operator's figures on a port of their own (`monitoring.metrics_listen`):
//! `GET /metrics` in Prometheus's text format and `GET /ready`
//! ([`crate::readiness`]). Nothing on the node's public port but `/ready`,
//! which the balancer needs; the figures name tenants and stay inside.
//!
//! The figures are this node's: whether it is ready, how far each read model
//! trails the store, its workers by state, the writes it keeps for nodes
//! that are away, the nodes it gave up on, its live store connections by
//! tenant, the size of the store's files, and the consensus engine's own
//! counters (evento's), every series
//! labelled with the node's id so one scrape of three nodes tells them apart.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use topcoat::context::{Cx, context};
use topcoat::router::{
    Body, HeaderValue, Method, RouteFn, RouteFuture, RouterBuilder, header::CONTENT_TYPE,
    response::Response,
};

use crate::readiness::{Readiness, Report, ready_route};
use crate::workers::WorkerState;

/// The metrics server, until shut down.
pub struct MetricsServer {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl MetricsServer {
    /// Stops taking requests and waits for the server to end.
    pub async fn shutdown(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

/// Where the store's file is, for the sizes `/metrics` gives.
#[derive(Clone)]
struct StorePath(Arc<PathBuf>);

/// Serves `/metrics` and `/ready` on `listen`; `store` is the store's file.
pub async fn serve(
    listen: SocketAddr,
    readiness: Arc<Readiness>,
    store: &Path,
) -> Result<MetricsServer> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot bind the metrics listener on {listen}"))?;
    tracing::info!(addr = %listener.local_addr()?, "metrics listening");
    let router = RouterBuilder::new()
        .route(RouteFn::new(Method::GET, "/metrics", metrics_route))
        .route(RouteFn::new(Method::GET, "/ready", ready_route))
        .with(readiness)
        .with(StorePath(Arc::new(store.to_path_buf())))
        .build();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let signal = async {
            let _ = stopped.await;
        };
        if let Err(err) = topcoat::serve_until(listener, router, signal).await {
            tracing::error!(%err, "the metrics server ended");
        }
    });
    Ok(MetricsServer { stop, task })
}

/// `GET /metrics`.
fn metrics_route(cx: &Cx, _body: Body) -> RouteFuture<'_> {
    Box::pin(async move {
        let readiness = context::<Arc<Readiness>>(cx);
        let report = readiness
            .report()
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        let store = context::<StorePath>(cx);
        let mut body = render(readiness, &report);
        body.push_str(&render_store(
            &readiness.cluster().health().id.0.to_string(),
            store_sizes(&store.0),
        ));
        let mut response = Response::new(Body::from(body));
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        );
        Ok(response)
    })
}

/// The figures in Prometheus's text format: this application's, then the
/// consensus engine's.
fn render(readiness: &Readiness, report: &Report) -> String {
    let cluster = readiness.cluster();
    let health = cluster.health();
    let node = health.id.0.to_string();
    let mut out = String::new();
    let _ = writeln!(out, "# TYPE timada_ready gauge");
    let _ = writeln!(
        out,
        "timada_ready{{node=\"{node}\"}} {}",
        u8::from(report.ready)
    );
    let _ = writeln!(out, "# TYPE timada_read_model_lag_seconds gauge");
    for lag in report.read_models.iter().chain(&report.per_node) {
        if let Some(seconds) = lag.lag_seconds {
            let _ = writeln!(
                out,
                "timada_read_model_lag_seconds{{node=\"{node}\",read_model=\"{}\"}} {seconds}",
                lag.name
            );
        }
    }
    let _ = writeln!(out, "# TYPE timada_read_model_stuck_seconds gauge");
    let _ = writeln!(
        out,
        "timada_read_model_stuck_seconds{{node=\"{node}\"}} {}",
        report.stuck_seconds.unwrap_or(0)
    );
    let _ = writeln!(out, "# TYPE timada_workers gauge");
    let states = readiness.worker_states();
    for (word, matches) in [
        (
            "running",
            (|s: &WorkerState| s.is_running()) as fn(&WorkerState) -> bool,
        ),
        ("restarting", |s| {
            matches!(s, WorkerState::Restarting { .. })
        }),
        ("stopped", |s| matches!(s, WorkerState::Stopped { .. })),
    ] {
        let count = states.iter().filter(|(_, state)| matches(state)).count();
        let _ = writeln!(
            out,
            "timada_workers{{node=\"{node}\",state=\"{word}\"}} {count}"
        );
    }
    let (kept, limit) = health.backlog;
    let _ = writeln!(out, "# TYPE timada_write_backlog_kept gauge");
    let _ = writeln!(out, "timada_write_backlog_kept{{node=\"{node}\"}} {kept}");
    let _ = writeln!(out, "# TYPE timada_write_backlog_limit gauge");
    let _ = writeln!(out, "timada_write_backlog_limit{{node=\"{node}\"}} {limit}");
    let given_up = health.peers.iter().filter(|peer| peer.given_up).count();
    let _ = writeln!(out, "# TYPE timada_peers_given_up gauge");
    let _ = writeln!(out, "timada_peers_given_up{{node=\"{node}\"}} {given_up}");
    let in_touch = health
        .peers
        .iter()
        .filter(|peer| crate::cluster::in_touch(peer))
        .count();
    let _ = writeln!(out, "# TYPE timada_peers_in_touch gauge");
    let _ = writeln!(out, "timada_peers_in_touch{{node=\"{node}\"}} {in_touch}");
    let _ = writeln!(out, "# TYPE timada_store_connections gauge");
    let mut connections: Vec<(String, usize)> =
        readiness.connections_by_tenant().into_iter().collect();
    connections.sort();
    for (tenant, count) in connections {
        let _ = writeln!(
            out,
            "timada_store_connections{{node=\"{node}\",tenant=\"{}\"}} {count}",
            escape(&tenant)
        );
    }
    out.push_str(
        &cluster
            .node()
            .metrics()
            .to_prometheus_labeled(&[("node", &node)]),
    );
    out
}

/// The sizes of the store's file, its write-ahead log and its commit log
/// (`db::store_files`), a file that is not there counting for nothing.
fn store_sizes(store: &Path) -> [u64; 3] {
    crate::db::store_files(store).map(|file| std::fs::metadata(file).map_or(0, |m| m.len()))
}

/// The store's files by size. The file itself only grows; the two logs are
/// emptied at each checkpoint (a few megabytes apart), so one that keeps
/// growing says the checkpoints fail.
fn render_store(node: &str, sizes: [u64; 3]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# TYPE timada_store_bytes gauge");
    for (file, bytes) in ["database", "write_ahead_log", "commit_log"]
        .iter()
        .zip(sizes)
    {
        let _ = writeln!(
            out,
            "timada_store_bytes{{node=\"{node}\",file=\"{file}\"}} {bytes}"
        );
    }
    out
}

/// A label value, as the text format wants it.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(escape("plain"), "plain");
        assert_eq!(escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn the_store_s_files_are_sized_one_by_one() {
        let dir = std::env::temp_dir().join(format!("timada-metrics-{}", ulid::Ulid::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("timada.db");
        std::fs::write(&store, [0u8; 10]).unwrap();
        std::fs::write(crate::db::with_suffix(&store, "-log"), [0u8; 3]).unwrap();
        // No write-ahead log yet: it counts for nothing.
        assert_eq!(store_sizes(&store), [10, 0, 3]);
        assert_eq!(
            render_store("2", [10, 0, 3]),
            "# TYPE timada_store_bytes gauge\n\
             timada_store_bytes{node=\"2\",file=\"database\"} 10\n\
             timada_store_bytes{node=\"2\",file=\"write_ahead_log\"} 0\n\
             timada_store_bytes{node=\"2\",file=\"commit_log\"} 3\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
