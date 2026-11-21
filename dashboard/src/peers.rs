//! The other nodes of the cluster, as the files every node keeps reach them:
//! the product pictures ([`crate::media`]) and the personal-data keys
//! ([`crate::key_vault`]). A file is copied to a majority of the cluster
//! before the event naming it is recorded, the same majority a commit needs,
//! and each of those nodes syncs the file to its disk before it answers
//! ([`crate::disk`]), as the store syncs each write: the file survives
//! whatever the events survive, a power cut across the cluster included. The
//! rate limits ask the same nodes what they let through
//! ([`crate::rate_limit`]).
//!
//! They are asked on the port each keeps for it (`cluster.web_listen`),
//! where nothing else is served ([`crate::peer_port`]), over mutual TLS with
//! this node's certificate: a node is let in as the node its certificate
//! names, as on the store's port, and no secret travels.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use anyhow::Context;
use evento::cluster::NodeTls;
use evento::cluster::tls::tokio_rustls::rustls::ClientConfig;
use openidconnect::reqwest;

use crate::config::ClusterConfig;

/// How long one copy to or from a peer may take.
const PEER_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Peers {
    /// Web addresses of the other nodes, no trailing slash.
    urls: Vec<String>,
    /// Copies the other nodes must acknowledge before a file is recorded:
    /// with this node's own, a majority of the cluster.
    quorum: usize,
    client: Client,
}

/// What the requests to the other nodes go through.
enum Client {
    /// Mutual TLS with this node's certificate, built again once the
    /// certificate is renewed, which the node reads again as it runs.
    Tls { tls: NodeTls, built: RwLock<Built> },
    /// Plain HTTP, to the tests' stand-in peers.
    #[cfg(test)]
    Plain(reqwest::Client),
}

/// A client, and the TLS it was built with.
struct Built {
    config: Arc<ClientConfig>,
    client: reqwest::Client,
}

impl Peers {
    /// The other nodes, reached with this node's certificate `tls`, or
    /// `None` for a single node.
    pub fn from_cluster(
        cluster: Option<&ClusterConfig>,
        tls: Option<&NodeTls>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(cluster) = cluster.filter(|cluster| cluster.peers.len() > 1) else {
            return Ok(None);
        };
        // Required by `Config::load` with more than one node.
        let tls = tls
            .context("the nodes of a cluster talk over TLS: cluster.tls")?
            .clone();
        let urls = cluster
            .web_peers
            .iter()
            .enumerate()
            .filter(|(index, _)| *index as u64 != cluster.node_id)
            .map(|(_, url)| url.clone())
            .collect();
        let config = tls.connector().config().clone();
        let client = build(&config)?;
        Ok(Some(Self {
            urls,
            quorum: cluster.peers.len() / 2,
            client: Client::Tls {
                tls,
                built: RwLock::new(Built { config, client }),
            },
        }))
    }

    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    /// The client a request to a peer goes through, presenting this node's
    /// certificate as it stands.
    pub fn client(&self) -> reqwest::Client {
        match &self.client {
            Client::Tls { tls, built } => {
                let config = tls.connector().config().clone();
                {
                    let built = built.read().unwrap_or_else(PoisonError::into_inner);
                    if Arc::ptr_eq(&built.config, &config) {
                        return built.client.clone();
                    }
                }
                // Renewed: the requests from now on present the new one.
                let mut built = built.write().unwrap_or_else(PoisonError::into_inner);
                if !Arc::ptr_eq(&built.config, &config) {
                    match build(&config) {
                        Ok(client) => built.client = client,
                        Err(err) => tracing::warn!(
                            error = %format!("{err:#}"),
                            "cannot reach the other nodes with the renewed certificate; the requests go on with the one before"
                        ),
                    }
                    // Once a renewal: a failure is not tried again each time.
                    built.config = config;
                }
                built.client.clone()
            }
            #[cfg(test)]
            Client::Plain(client) => client.clone(),
        }
    }

    /// PUTs `body` at `path` on every other node, returning once a majority
    /// of the cluster (this node included) holds it. The copies to the
    /// remaining nodes carry on in the background. Fails when too many nodes
    /// refused or could not be reached for a majority; what names the file
    /// must then not be recorded. `what` names the file in messages.
    pub async fn put_to_quorum(
        &self,
        path: &str,
        body: bytes::Bytes,
        what: &'static str,
    ) -> anyhow::Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(self.urls.len());
        let client = self.client();
        for url in &self.urls {
            let request = client.put(format!("{url}{path}")).body(body.clone());
            let (tx, url) = (tx.clone(), url.clone());
            tokio::spawn(async move {
                let copied = match request.send().await {
                    Ok(response) if response.status().is_success() => Ok(()),
                    Ok(response) => Err(anyhow::anyhow!("answered {}", response.status())),
                    Err(err) => Err(err.into()),
                };
                if let Err(err) = &copied {
                    tracing::warn!(peer = %url, %err, what, "cannot copy a file to a peer");
                }
                // The upload may already have its majority and be gone.
                let _ = tx.send(copied.is_ok()).await;
            });
        }
        drop(tx);
        let (mut copied, mut failed) = (0, 0);
        while let Some(ok) = rx.recv().await {
            if ok {
                copied += 1;
            } else {
                failed += 1;
            }
            if copied >= self.quorum {
                return Ok(());
            }
            if failed > self.urls.len() - self.quorum {
                break;
            }
        }
        anyhow::bail!(
            "only {copied} of {} other nodes took the {what}, {} needed",
            self.urls.len(),
            self.quorum
        )
    }
}

/// A client presenting this node's certificate, as `config` holds it.
fn build(config: &ClientConfig) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .use_preconfigured_tls(config.clone())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PEER_TIMEOUT)
        .build()
        .context("cannot build the client for the other nodes")
}

/// A tenant, picture or key id: letters, digits, `-` and `_`, so it names one
/// entry of a directory and nothing above it.
pub fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use super::*;

    impl Peers {
        /// These peers, in a cluster of `nodes`, over plain HTTP.
        pub fn for_test(urls: Vec<String>, nodes: usize) -> Self {
            Self {
                urls,
                quorum: nodes / 2,
                client: Client::Plain(reqwest::Client::new()),
            }
        }

        /// These peers, in a cluster of `nodes`, reached over mutual TLS with
        /// `tls`.
        pub fn for_test_over(urls: Vec<String>, nodes: usize, tls: NodeTls) -> Self {
            let config = tls.connector().config().clone();
            let client = build(&config).unwrap();
            Self {
                urls,
                quorum: nodes / 2,
                client: Client::Tls {
                    tls,
                    built: RwLock::new(Built { config, client }),
                },
            }
        }
    }

    /// A peer answering every request with `status` and `body`.
    pub async fn peer(status: u16, body: &'static [u8]) -> String {
        answering(move |_, _| (status, body.to_vec())).await
    }

    /// A peer answering every request with what `answer` makes of its head
    /// (the request line and headers, as sent) and body.
    pub async fn answering(
        answer: impl Fn(&str, &[u8]) -> (u16, Vec<u8>) + Send + Sync + 'static,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let answer = Arc::new(answer);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let answer = answer.clone();
                tokio::spawn(async move {
                    // Read the head, then as much body as it announces.
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head_end, length) = loop {
                        let read = stream.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&chunk[..read]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                            let length = head
                                .lines()
                                .find_map(|line| line.strip_prefix("content-length:"))
                                .and_then(|value| value.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            break (end, end + 4 + length);
                        }
                    };
                    while request.len() < length {
                        let read = stream.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&chunk[..read]);
                    }
                    let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
                    let (status, body) =
                        answer(&head, &request[(head_end + 4).min(request.len())..]);
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        format!("http://{addr}")
    }

    /// An address nothing listens on.
    pub async fn down() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    /// A peer that takes every connection and never answers.
    pub async fn silent() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        format!("http://{addr}")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    async fn put(peers: Peers) -> anyhow::Result<()> {
        peers
            .put_to_quorum("/cluster/media/t1/i1.png", bytes::Bytes::new(), "picture")
            .await
    }

    #[tokio::test]
    async fn a_copy_needs_a_majority_of_the_cluster() {
        // Three nodes: this one and one peer are a majority.
        let three = Peers::for_test(vec![peer(204, b"").await, down().await], 3);
        assert!(put(three).await.is_ok());

        // This one alone is not.
        let alone = Peers::for_test(vec![peer(401, b"").await, down().await], 3);
        assert!(put(alone).await.is_err());

        // Five nodes: two peers more are needed.
        let five_ok = Peers::for_test(
            vec![
                peer(204, b"").await,
                peer(204, b"").await,
                down().await,
                peer(500, b"").await,
            ],
            5,
        );
        assert!(put(five_ok).await.is_ok());
        let five_short = Peers::for_test(
            vec![
                peer(204, b"").await,
                down().await,
                down().await,
                peer(422, b"").await,
            ],
            5,
        );
        assert!(put(five_short).await.is_err());
    }

    #[test]
    fn an_id_names_one_entry() {
        assert!(is_id("t-1_A"));
        for id in ["", "..", "t/1", "a.b", &"x".repeat(65)] {
            assert!(!is_id(id), "{id}");
        }
    }
}
