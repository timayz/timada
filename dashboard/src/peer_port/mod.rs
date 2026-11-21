//! The port each node keeps for the other nodes, `cluster.web_listen`: what
//! they ask of it besides the store ([`cluster`]), and nothing else. The
//! public port serves none of these routes, so no request from outside the
//! cluster reaches them.
//!
//! A connection is mutual TLS with this node's certificate, the one the
//! store's port presents ([`NodeTls`]), and is taken from a node of the
//! cluster only: the handshake asks for a certificate issued by the
//! cluster's authority, and that certificate must name a member as the
//! store's port dials it ([`Members`]), the check the store's port lets a
//! node in by ([`tls::names`]). Anything else ends before a request is read.

pub mod cluster;

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use evento::cluster::tls::tokio_rustls::rustls::pki_types::CertificateDer;
use evento::cluster::tls::tokio_rustls::server::TlsStream;
use evento::cluster::{NodeTls, tls};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use topcoat::router::{
    Body, BodyLimit, LayerFn, LayerFuture, Listener, Next, RouterService, internal_serve,
    module_router,
};

use crate::app::server_errors::LogServerErrors;
use crate::cluster::Cluster;
use crate::key_vault::{self, KeyVault};
use crate::media::{self, Media};
use crate::rate_limit::RateLimiter;

/// How long the requests under way get to finish once the node stops.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a connection may take to say which node it is.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a refused connection is logged, at most: a node with a wrong
/// file tries again every few seconds.
const SAY_REFUSED_EVERY: Duration = Duration::from_secs(30);

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

/// Binds `listen` and serves the other nodes' requests there over `tls`:
/// copies of pictures and keys, and the limits. A node is let in as a member
/// of `cluster`.
pub async fn serve(
    listen: SocketAddr,
    tls: NodeTls,
    cluster: Arc<Cluster>,
    media: Arc<Media>,
    keys: Arc<KeyVault>,
    limits: Arc<RateLimiter>,
) -> Result<PeerServer> {
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot bind the port of the nodes on {listen}"))?;
    tracing::info!(addr = %listener.local_addr()?, "listening for the other nodes");
    let nodes = NodesOnly::new(listener, tls, Arc::new(Members(cluster)));
    Ok(serve_on(nodes, media, keys, limits))
}

/// The same on connections taken already.
fn serve_on(
    listener: impl Listener,
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

/// Who is let in on the nodes' port, by the certificates a connection
/// presented (its own first), which the handshake checked against the
/// cluster's authority.
trait Admits: Send + Sync + 'static {
    fn admits(&self, presented: Option<&[CertificateDer<'_>]>) -> bool;
}

/// The members of the cluster, and the nodes it is changing to: a
/// certificate valid for the address the store's port dials one at, as the
/// store's port lets a node in. A node added while this one runs is let in
/// once the cluster has its address, and a node removed no longer is.
struct Members(Arc<Cluster>);

impl Admits for Members {
    fn admits(&self, presented: Option<&[CertificateDer<'_>]>) -> bool {
        let members = self.0.members();
        let changing_to = members.change.iter().flat_map(|change| change.to.iter());
        members
            .nodes
            .iter()
            .chain(changing_to)
            .filter_map(|node| self.0.address(*node))
            .any(|address| tls::names(presented, &address).is_ok())
    }
}

/// The connections of the nodes' port: each handshake on a task of its own,
/// so a slow one holds up no other, and only those `admits` lets in handed
/// to the server.
struct NodesOnly {
    taken: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    task: JoinHandle<()>,
}

impl NodesOnly {
    fn new(listener: TcpListener, tls: NodeTls, admits: Arc<dyn Admits>) -> Self {
        let (tx, taken) = mpsc::channel(64);
        let task = tokio::spawn(take_connections(listener, tls, admits, tx));
        Self { taken, task }
    }
}

impl Drop for NodesOnly {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Listener for NodesOnly {
    type Io = TlsStream<TcpStream>;

    async fn accept(&mut self) -> io::Result<(Self::Io, Option<SocketAddr>)> {
        let (stream, from) =
            self.taken.recv().await.ok_or_else(|| {
                io::Error::other("the port of the nodes takes no more connections")
            })?;
        Ok((stream, Some(from)))
    }
}

/// Takes the connections on `listener` and hands on those of the nodes. A
/// failure to take one (no file descriptor left, say) is waited out: the
/// port stays open.
async fn take_connections(
    listener: TcpListener,
    tls: NodeTls,
    admits: Arc<dyn Admits>,
    tx: mpsc::Sender<(TlsStream<TcpStream>, SocketAddr)>,
) {
    let refusals = Arc::new(Refusals::default());
    loop {
        let (tcp, from) = match listener.accept().await {
            Ok(taken) => taken,
            Err(err) => {
                tracing::warn!(%err, "the port of the nodes cannot take a connection");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        // The certificate as it stands: one renewed is presented from the
        // next connection on.
        let acceptor = tls.acceptor();
        let (admits, tx, refusals) = (admits.clone(), tx.clone(), refusals.clone());
        tokio::spawn(async move {
            let stream = match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(err)) => {
                    return refusals.say(from, &format!("the TLS handshake failed: {err}"));
                }
                Err(_) => return refusals.say(from, "the TLS handshake took too long"),
            };
            if !admits.admits(stream.get_ref().1.peer_certificates()) {
                return refusals.say(from, "its certificate names no node of the cluster");
            }
            // The server may have stopped meanwhile.
            let _ = tx.send((stream, from)).await;
        });
    }
}

/// When a refused connection was last logged.
#[derive(Default)]
struct Refusals(Mutex<Option<Instant>>);

impl Refusals {
    fn say(&self, from: SocketAddr, why: &str) {
        let mut last = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|last| last.elapsed() < SAY_REFUSED_EVERY) {
            return;
        }
        *last = Some(Instant::now());
        tracing::warn!(%from, why, "a connection on the port of the nodes is refused");
    }
}

#[cfg(test)]
mod tests {
    use evento::cluster::tls::tokio_rustls::rustls::pki_types::pem::PemObject;
    use evento::cluster::tls::tokio_rustls::rustls::{self, ClientConfig, RootCertStore};
    use openidconnect::reqwest;
    use timada_core::Rendition;

    use super::*;
    use crate::config::DataKeysConfig;
    use crate::media::test_support::{png, rendered};
    use crate::peers::Peers;

    const MINUTE: Duration = Duration::from_secs(60);

    /// The authority of a test cluster.
    struct Authority {
        params: rcgen::CertificateParams,
        key: rcgen::KeyPair,
        pem: String,
    }

    impl Authority {
        fn new() -> Self {
            let key = rcgen::KeyPair::generate().unwrap();
            let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
            params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            let pem = params.self_signed(&key).unwrap().pem();
            Self { params, key, pem }
        }

        /// A certificate for `name` (a host, or an address) and its key.
        fn issue(&self, name: &str) -> (String, String) {
            let key = rcgen::KeyPair::generate().unwrap();
            let params = rcgen::CertificateParams::new(vec![name.to_owned()]).unwrap();
            let issuer = rcgen::Issuer::from_params(&self.params, &self.key);
            let cert = params.signed_by(&key, &issuer).unwrap();
            (cert.pem(), key.serialize_pem())
        }

        /// A node's side of TLS, with a certificate for `name`.
        fn node(&self, name: &str) -> NodeTls {
            let (cert, key) = self.issue(name);
            NodeTls::from_pem(self.pem.as_bytes(), cert.as_bytes(), key.as_bytes()).unwrap()
        }
    }

    /// The nodes of the test cluster: those whose certificate names
    /// 127.0.0.1, where they all are.
    struct Loopback;

    impl Admits for Loopback {
        fn admits(&self, presented: Option<&[CertificateDer<'_>]>) -> bool {
            tls::names(presented, &"127.0.0.1:7000".parse().unwrap()).is_ok()
        }
    }

    /// The port of a node keeping `media` and counting with `limits`, over
    /// `tls`, and its address as the other nodes reach it.
    async fn port(
        tls: NodeTls,
        media: Arc<Media>,
        limits: Arc<RateLimiter>,
    ) -> (PeerServer, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        let dir = std::env::temp_dir().join(format!("timada-keys-{}", ulid::Ulid::generate()));
        let keys = Arc::new(KeyVault::new(&DataKeysConfig { dir }, None, None));
        let nodes = NodesOnly::new(listener, tls, Arc::new(Loopback));
        (serve_on(nodes, media, keys, limits), url)
    }

    /// The other nodes at `urls`, in a cluster of `nodes`, reached over `tls`.
    fn peers(urls: Vec<String>, nodes: usize, tls: NodeTls) -> Peers {
        Peers::for_test_over(urls, nodes, tls)
    }

    /// A client trusting `authority`, presenting `tls`'s certificate or none.
    fn client(authority: &Authority, tls: Option<&NodeTls>) -> reqwest::Client {
        let config = match tls {
            Some(tls) => tls.connector().config().as_ref().clone(),
            None => {
                let mut roots = RootCertStore::empty();
                for cert in CertificateDer::pem_slice_iter(authority.pem.as_bytes()) {
                    roots.add(cert.unwrap()).unwrap();
                }
                ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::aws_lc_rs::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth()
            }
        };
        reqwest::Client::builder()
            .use_preconfigured_tls(config)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn a_picture_is_copied_to_a_nodes_port_and_fetched_from_it() {
        let authority = Authority::new();
        let b = Arc::new(crate::media::tests::media(vec![], 3));
        let limits = Arc::new(RateLimiter::new(None));
        let (server, b_url) = port(authority.node("127.0.0.1"), b.clone(), limits).await;

        // A keeps an upload: it is recorded only once B took the copies.
        let a = crate::media::tests::media_over(peers(
            vec![b_url.clone()],
            2,
            authority.node("127.0.0.1"),
        ));
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
        let c = crate::media::tests::media_over(peers(vec![b_url], 2, authority.node("127.0.0.1")));
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
        let authority = Authority::new();
        // B let "k" through twice.
        let b = Arc::new(RateLimiter::new(None));
        assert!(b.check("k", 10, MINUTE).await);
        assert!(b.check("k", 10, MINUTE).await);
        let media = Arc::new(crate::media::tests::media(vec![], 3));
        let (server, b_url) = port(authority.node("127.0.0.1"), media, b).await;

        // A counts them with its own: a third pass is past a limit of two.
        let a = RateLimiter::new(Some(Arc::new(peers(
            vec![b_url],
            3,
            authority.node("127.0.0.1"),
        ))));
        assert!(!a.check("k", 2, MINUTE).await);
        assert!(a.check("other", 2, MINUTE).await);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn the_port_lets_in_the_nodes_of_the_cluster_and_nothing_else() {
        let authority = Authority::new();
        let media = Arc::new(crate::media::tests::media(vec![], 3));
        let limits = Arc::new(RateLimiter::new(None));
        let (server, url) = port(authority.node("127.0.0.1"), media, limits).await;
        let keys = format!("{url}/cluster/keys");
        let status = |client: reqwest::Client, url: String| async move {
            client
                .get(url)
                .send()
                .await
                .map(|answer| answer.status().as_u16())
        };

        // A node of the cluster.
        let node = client(&authority, Some(&authority.node("127.0.0.1")));
        assert_eq!(status(node.clone(), keys.clone()).await.unwrap(), 200);
        // Nothing but the nodes' routes is served there.
        assert_eq!(
            status(node.clone(), format!("{url}/ready")).await.unwrap(),
            404
        );
        assert_eq!(
            status(node, format!("{url}/media/acme/400/i1.webp"))
                .await
                .unwrap(),
            404
        );

        // No certificate, a certificate of the authority naming no node, one
        // of another authority, or no TLS at all: no answer.
        let stranger = client(&authority, Some(&authority.node("stranger.example")));
        let other = Authority::new();
        let foreign = client(&authority, Some(&other.node("127.0.0.1")));
        for (who, client) in [
            ("no certificate", client(&authority, None)),
            ("a stranger", stranger),
            ("another authority", foreign),
        ] {
            assert!(status(client, keys.clone()).await.is_err(), "{who}");
        }
        let in_clear = keys.replacen("https://", "http://", 1);
        assert!(status(reqwest::Client::new(), in_clear).await.is_err());
        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_renewed_certificate_is_presented_from_the_next_request_on() {
        let authority = Authority::new();
        let media = Arc::new(crate::media::tests::media(vec![], 3));
        let limits = Arc::new(RateLimiter::new(Some(Arc::new(Peers::for_test(vec![], 3)))));
        let (server, url) = port(authority.node("127.0.0.1"), media, limits).await;
        let tls = authority.node("127.0.0.1");
        let a = peers(vec![url.clone()], 3, tls.clone());
        let asks = |a: &Peers| {
            let request = a.client().get(format!("{url}/cluster/keys"));
            async move { request.send().await.is_ok() }
        };
        assert!(asks(&a).await);

        // Renewed for a name that is no node's: refused, with no restart.
        let (cert, key) = authority.issue("stranger.example");
        assert!(
            tls.replace(authority.pem.as_bytes(), cert.as_bytes(), key.as_bytes())
                .unwrap()
        );
        assert!(!asks(&a).await);
        // And renewed again for its own: let in.
        let (cert, key) = authority.issue("127.0.0.1");
        assert!(
            tls.replace(authority.pem.as_bytes(), cert.as_bytes(), key.as_bytes())
                .unwrap()
        );
        assert!(asks(&a).await);
        server.shutdown().await;
    }
}
