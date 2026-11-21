//! The port each node keeps for the other nodes, `cluster.web_listen`: what
//! they ask of it besides the store ([`cluster`]), and nothing else. The
//! public port serves none of these routes, so no request from outside the
//! cluster reaches them.

pub mod cluster;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use topcoat::router::{
    Body, BodyLimit, LayerFn, LayerFuture, Next, RouterService, internal_serve, module_router,
};

use crate::app::server_errors::LogServerErrors;
use crate::key_vault::{self, KeyVault};
use crate::media::{self, Media};
use crate::rate_limit::RateLimiter;

/// How long the requests under way get to finish once the node stops.
const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The nodes' port, served until [`shutdown`](Self::shutdown).
pub struct PeerServer {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl PeerServer {
    /// Stops taking requests and waits for those under way.
    pub async fn shutdown(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

/// Binds `listen` and serves the other nodes' requests there: copies of
/// pictures and keys, and the limits.
pub async fn serve(
    listen: SocketAddr,
    media: Arc<Media>,
    keys: Arc<KeyVault>,
    limits: Arc<RateLimiter>,
) -> Result<PeerServer> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot bind the port of the nodes on {listen}"))?;
    tracing::info!(addr = %listener.local_addr()?, "listening for the other nodes");
    Ok(serve_on(listener, media, keys, limits))
}

/// The same on a listener bound already.
fn serve_on(
    listener: tokio::net::TcpListener,
    media: Arc<Media>,
    keys: Arc<KeyVault>,
    limits: Arc<RateLimiter>,
) -> PeerServer {
    // The routes below this module, and only those.
    let router = module_router!()
        // A rendition of a picture, as the node that took the upload made it.
        .layer(body_limit(
            media::PEER_ROUTE,
            media::MAX_RENDITION_BYTES as usize,
        ))
        .layer(body_limit(key_vault::PEER_ROUTE, key_vault::MAX_SEALED_LEN))
        .with(media)
        .with(keys)
        .with(limits)
        .layer(LogServerErrors)
        .build();
    let service = RouterService::new(router).shutdown_timeout(SHUTDOWN_TIMEOUT);
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let signal = async {
            let _ = stopped.await;
        };
        // Not `topcoat::serve_until`: under `topcoat dev`, that would tell
        // the dev server to send the pages here.
        if let Err(err) = internal_serve(listener, service, signal).await {
            tracing::error!(%err, "the port of the nodes stopped");
        }
    });
    PeerServer { stop, task }
}

/// Sets the body limit to `max` bytes for the routes under `path`.
fn body_limit(path: &'static str, max: usize) -> LayerFn {
    fn pass<'a>(cx: &'a topcoat::context::Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        next.run(cx, body)
    }
    LayerFn::new(Some(path), pass).with(BodyLimit::max(max))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use openidconnect::reqwest;
    use timada_core::Rendition;

    use super::*;
    use crate::config::DataKeysConfig;
    use crate::media::test_support::{png, rendered};
    use crate::peers::Peers;
    use crate::peers::test_support::SECRET;

    const MINUTE: Duration = Duration::from_secs(60);

    /// The other nodes, as a node of `nodes` that asks those at `urls`.
    fn peers(urls: Vec<String>, nodes: usize) -> Option<Arc<Peers>> {
        Some(Arc::new(Peers::for_test(urls, nodes)))
    }

    /// The port of a node keeping `media` and counting with `limits`, and
    /// its address as the other nodes reach it.
    async fn port(media: Arc<Media>, limits: Arc<RateLimiter>) -> (PeerServer, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let dir = std::env::temp_dir().join(format!("timada-keys-{}", ulid::Ulid::generate()));
        let keys = Arc::new(KeyVault::new(
            &DataKeysConfig { dir },
            None,
            peers(vec![], 3),
        ));
        (serve_on(listener, media, keys, limits), url)
    }

    #[tokio::test]
    async fn a_picture_is_copied_to_a_nodes_port_and_fetched_from_it() {
        let b = Arc::new(crate::media::tests::media(vec![], 3));
        let (server, b_url) = port(b.clone(), Arc::new(RateLimiter::new(peers(vec![], 3)))).await;

        // A keeps an upload: it is recorded only once B took the copies.
        let a = crate::media::tests::media(vec![b_url.clone()], 2);
        let picture = a
            .keep("acme", "i1", rendered(&png(900, 600, [7, 7, 7, 255])))
            .await
            .unwrap();
        for rendition in Rendition::ALL {
            let kept = std::fs::read(a.path_of("acme", rendition, "i1").unwrap()).unwrap();
            let copied = std::fs::read(b.path_of("acme", rendition, "i1").unwrap()).unwrap();
            assert_eq!(copied, kept, "{rendition:?}");
        }

        // C lacks it, and fetches it from B.
        let c = crate::media::tests::media(vec![b_url], 2);
        c.fetch("acme", "i1", &picture).await.unwrap();
        for rendition in Rendition::ALL {
            let fetched = std::fs::read(c.path_of("acme", rendition, "i1").unwrap()).unwrap();
            let held = std::fs::read(b.path_of("acme", rendition, "i1").unwrap()).unwrap();
            assert_eq!(fetched, held, "{rendition:?}");
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_node_asks_the_limits_on_the_others_port() {
        // B let "k" through twice.
        let b = Arc::new(RateLimiter::new(peers(vec![], 3)));
        assert!(b.check("k", 10, MINUTE).await);
        assert!(b.check("k", 10, MINUTE).await);
        let (server, b_url) = port(Arc::new(crate::media::tests::media(vec![], 3)), b).await;

        // A counts them with its own: a third pass is past a limit of two.
        let a = RateLimiter::new(peers(vec![b_url], 3));
        assert!(!a.check("k", 2, MINUTE).await);
        assert!(a.check("other", 2, MINUTE).await);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn the_port_serves_only_the_nodes_and_their_routes() {
        let (server, url) = port(
            Arc::new(crate::media::tests::media(vec![], 3)),
            Arc::new(RateLimiter::new(peers(vec![], 3))),
        )
        .await;
        let client = reqwest::Client::new();
        let picture = format!("{url}/cluster/media/acme/400/i1.webp");
        let status = |request: reqwest::RequestBuilder| async move {
            request.send().await.unwrap().status().as_u16()
        };

        // Without the cluster's secret, nothing.
        assert_eq!(status(client.put(&picture).body("x")).await, 401);
        assert_eq!(status(client.get(&picture)).await, 401);
        assert_eq!(status(client.get(format!("{url}/cluster/keys"))).await, 401);
        assert_eq!(
            status(client.post(format!("{url}/cluster/limits")).body("{}")).await,
            401
        );

        // With it, the routes answer.
        let with_secret = |request: reqwest::RequestBuilder| {
            request.header(reqwest::header::AUTHORIZATION, SECRET)
        };
        assert_eq!(status(with_secret(client.get(&picture))).await, 404);
        assert_eq!(
            status(with_secret(client.get(format!("{url}/cluster/keys")))).await,
            200
        );

        // And nothing else is served there.
        assert_eq!(status(client.get(format!("{url}/ready"))).await, 404);
        assert_eq!(
            status(client.get(format!("{url}/media/acme/400/i1.webp"))).await,
            404
        );
        server.shutdown().await;
    }
}
