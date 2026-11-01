//! The other nodes of the cluster, as the files every node keeps reach them:
//! the product pictures ([`crate::media`]) and the shopper keys
//! ([`crate::key_vault`]). A file is copied to a majority of the cluster
//! before the event naming it is recorded, the same majority a commit needs,
//! so it survives whatever the events survive. The rate limits ask the same
//! nodes what they let through ([`crate::rate_limit`]).

use std::sync::Arc;
use std::time::Duration;

use openidconnect::reqwest;

use crate::config::ClusterConfig;

/// How long one copy to or from a peer may take.
const PEER_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Peers {
    /// Web addresses of the other nodes, no trailing slash.
    urls: Vec<String>,
    /// `Bearer <cluster.secret>`.
    authorization: String,
    /// Copies the other nodes must acknowledge before a file is recorded:
    /// with this node's own, a majority of the cluster.
    quorum: usize,
    client: reqwest::Client,
}

impl Peers {
    /// The other nodes, or `None` for a single node.
    pub fn from_cluster(cluster: Option<&ClusterConfig>) -> anyhow::Result<Option<Self>> {
        let Some(cluster) = cluster.filter(|cluster| cluster.peers.len() > 1) else {
            return Ok(None);
        };
        let urls = cluster
            .web_peers
            .iter()
            .enumerate()
            .filter(|(index, _)| *index as u64 != cluster.node_id)
            .map(|(_, url)| url.clone())
            .collect();
        // Validated by `Config::load`.
        let secret = cluster.secret.as_deref().unwrap_or_default();
        Ok(Some(Self {
            urls,
            authorization: format!("Bearer {secret}"),
            quorum: cluster.peers.len() / 2,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(PEER_TIMEOUT)
                .build()?,
        }))
    }

    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// The `Authorization` header a request to a peer carries.
    pub fn authorization(&self) -> &str {
        &self.authorization
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
        for url in &self.urls {
            let request = self
                .client
                .put(format!("{url}{path}"))
                .header(reqwest::header::AUTHORIZATION, &self.authorization)
                .body(body.clone());
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

    /// Whether a request carries the cluster's secret.
    pub fn authorizes(&self, header: Option<&[u8]>) -> bool {
        let Some(header) = header else {
            return false;
        };
        let expected = self.authorization.as_bytes();
        // Constant time, so the answer says nothing about how close a guess was.
        header.len() == expected.len()
            && header
                .iter()
                .zip(expected)
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }
}

/// Whether a request carries the cluster's secret. Without peers no request
/// does: a single node takes no copies.
pub fn authorizes(peers: Option<&Peers>, header: Option<&[u8]>) -> bool {
    peers.is_some_and(|peers| peers.authorizes(header))
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
    use super::*;

    /// The secret the peers built by [`Peers::for_test`] share.
    pub const SECRET: &str = "Bearer a-secret-shared-by-every-node-of-it";

    impl Peers {
        /// These peers, in a cluster of `nodes`.
        pub fn for_test(urls: Vec<String>, nodes: usize) -> Self {
            Self {
                urls,
                authorization: SECRET.into(),
                quorum: nodes / 2,
                client: reqwest::Client::new(),
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
    fn a_request_needs_the_secret() {
        let peers = Peers::for_test(vec![], 3);
        assert!(authorizes(Some(&peers), Some(SECRET.as_bytes())));
        assert!(!authorizes(
            Some(&peers),
            Some(b"Bearer a-secret-shared-by-every-node-of-iX")
        ));
        assert!(!authorizes(Some(&peers), Some(b"Bearer ")));
        assert!(!authorizes(Some(&peers), None));
        assert!(!authorizes(None, Some(SECRET.as_bytes())));
    }

    #[test]
    fn an_id_names_one_entry() {
        assert!(is_id("t-1_A"));
        for id in ["", "..", "t/1", "a.b", &"x".repeat(65)] {
            assert!(!is_id(id), "{id}");
        }
    }
}
