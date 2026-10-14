//! The pictures merchants upload for their products.
//!
//! The event store records what a picture is (`timada_catalog::ProductImage`);
//! the bytes live on disk under `media.dir`, one directory per tenant, named
//! after the picture's id. They are served from `/media/<tenant>/<id>.<ext>`
//! by topcoat's directory route, with no authentication: an id is an
//! unguessable ULID, and a shopper's browser has no session on this node.
//!
//! Storing before committing and deleting after means a failure leaves an
//! unreferenced file rather than a product pointing at nothing.
//!
//! With several nodes every node keeps every picture, so one that goes down
//! takes none with it. An upload copies the file to the other nodes and is
//! only recorded once a majority of nodes hold it, the same majority a
//! commit needs: a picture the catalog knows survives whatever the events
//! survive. Each node also follows the catalog's stream ([`subscription`])
//! and fetches from a peer any picture it lacks (a node that was down during
//! an upload, or a new one), and deletes the ones taken off a product.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use evento::subscription::{Context, SubscriptionBuilder};
use evento::{Executor, metadata::Event};
use openidconnect::reqwest;
use timada_catalog::aggregator::{ImageAdded, ImageRemoved};
use timada_catalog::{MAX_IMAGE_BYTES, product_projection};

use topcoat::context::Cx;
use topcoat::router::{
    Body, HeaderValue, IntoPath, Layer, LayerFuture, Next, Path as RoutePath,
    PathBuf as RoutePathBuf, header::CACHE_CONTROL, response::Response,
};

use crate::config::{ClusterConfig, MediaConfig};

/// The kinds of picture the shop takes, by extension.
const EXTENSIONS: [&str; 4] = ["jpg", "png", "webp", "gif"];

/// How long one copy to or from a peer may take.
const PEER_TIMEOUT: Duration = Duration::from_secs(30);

/// The route a node takes a copy of a picture on, below `/cluster/media`.
pub const PEER_ROUTE: &str = "/cluster/media";

/// Where the files are and the address they are served from.
pub struct Media {
    dir: PathBuf,
    public_url: String,
    /// The other nodes, when there are any.
    peers: Option<Peers>,
}

struct Peers {
    /// Web addresses of the other nodes, no trailing slash.
    urls: Vec<String>,
    /// `Bearer <cluster.secret>`.
    authorization: String,
    /// Copies the other nodes must acknowledge before an upload is recorded:
    /// with this node's own, a majority of the cluster.
    quorum: usize,
    client: reqwest::Client,
}

/// What a picture turned out to be, once its first bytes were read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SniffedImage {
    /// Lowercase, no dot.
    pub ext: &'static str,
    pub mime: &'static str,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Media {
    pub fn new(config: &MediaConfig, cluster: Option<&ClusterConfig>) -> anyhow::Result<Self> {
        let peers = match cluster {
            Some(cluster) if cluster.peers.len() > 1 => {
                let urls = cluster
                    .web_peers
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index as u64 != cluster.node_id)
                    .map(|(_, url)| url.clone())
                    .collect();
                // Validated by `Config::load`.
                let secret = cluster.secret.as_deref().unwrap_or_default();
                Some(Peers {
                    urls,
                    authorization: format!("Bearer {secret}"),
                    quorum: cluster.peers.len() / 2,
                    client: reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(PEER_TIMEOUT)
                        .build()?,
                })
            }
            _ => None,
        };
        Ok(Self {
            dir: config.dir.clone(),
            public_url: config.public_url.clone(),
            peers,
        })
    }

    /// The directory the files are served from.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where a tenant's pictures are loaded from, with no trailing slash:
    /// what its storefront is configured with.
    pub fn tenant_url(&self, tenant_id: &str) -> String {
        format!("{}/{tenant_id}", self.public_url)
    }

    /// The address of one picture.
    pub fn url(&self, tenant_id: &str, file: &str) -> String {
        format!("{}/{tenant_id}/{file}", self.public_url)
    }

    /// What the bytes are, or `None` when they are not a picture of a kind
    /// the shop accepts. Only the header is read, so an enormous file costs
    /// nothing to reject.
    pub fn sniff(bytes: &[u8]) -> Option<SniffedImage> {
        let (ext, mime) = match imagesize::image_type(bytes).ok()? {
            imagesize::ImageType::Jpeg => ("jpg", "image/jpeg"),
            imagesize::ImageType::Png => ("png", "image/png"),
            imagesize::ImageType::Webp => ("webp", "image/webp"),
            imagesize::ImageType::Gif => ("gif", "image/gif"),
            _ => return None,
        };
        let size = imagesize::blob_size(bytes).ok();
        Some(SniffedImage {
            ext,
            mime,
            width: size.as_ref().and_then(|s| u32::try_from(s.width).ok()),
            height: size.as_ref().and_then(|s| u32::try_from(s.height).ok()),
        })
    }

    /// Writes a picture. The bytes land in a temporary file first, so a
    /// reader never sees a half-written one.
    pub async fn store(
        &self,
        tenant_id: &str,
        image_id: &str,
        ext: &str,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        let dir = self.dir.join(tenant_id);
        tokio::fs::create_dir_all(&dir).await?;
        let partial = dir.join(format!("{image_id}.{ext}.part"));
        tokio::fs::write(&partial, bytes).await?;
        tokio::fs::rename(&partial, dir.join(format!("{image_id}.{ext}"))).await?;
        Ok(())
    }

    /// Copies a picture this node just stored to the other nodes, returning
    /// once a majority of the cluster (this node included) holds it. The
    /// copies to the remaining nodes carry on in the background. Fails when
    /// too many nodes refused or could not be reached for a majority; the
    /// picture must then not be recorded.
    pub async fn replicate(
        &self,
        tenant_id: &str,
        file: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        let Some(peers) = &self.peers else {
            return Ok(());
        };
        let bytes = bytes::Bytes::from(bytes);
        let (tx, mut rx) = tokio::sync::mpsc::channel(peers.urls.len());
        for url in &peers.urls {
            let request = peers
                .client
                .put(format!("{url}{PEER_ROUTE}/{tenant_id}/{file}"))
                .header(reqwest::header::AUTHORIZATION, &peers.authorization)
                .body(bytes.clone());
            let (tx, url) = (tx.clone(), url.clone());
            tokio::spawn(async move {
                let copied = match request.send().await {
                    Ok(response) if response.status().is_success() => Ok(()),
                    Ok(response) => Err(anyhow::anyhow!("answered {}", response.status())),
                    Err(err) => Err(err.into()),
                };
                if let Err(err) = &copied {
                    tracing::warn!(peer = %url, %err, "cannot copy a picture to a peer");
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
            if copied >= peers.quorum {
                return Ok(());
            }
            if failed > peers.urls.len() - peers.quorum {
                break;
            }
        }
        anyhow::bail!(
            "only {copied} of {} other nodes took the picture, {} needed",
            peers.urls.len(),
            peers.quorum
        )
    }

    /// Makes sure this node holds a picture the catalog references, fetching
    /// it from a peer that does when it is missing. The copy fetched must be
    /// the size the catalog recorded and a picture.
    pub async fn fetch(
        &self,
        tenant_id: &str,
        image_id: &str,
        ext: &str,
        size: u64,
    ) -> anyhow::Result<()> {
        let file = format!("{image_id}.{ext}");
        let path = self.dir.join(tenant_id).join(&file);
        if tokio::fs::metadata(&path)
            .await
            .is_ok_and(|meta| meta.len() == size)
        {
            return Ok(());
        }
        let Some(peers) = &self.peers else {
            anyhow::bail!(
                "picture {tenant_id}/{file} is missing and there is no peer to fetch it from"
            );
        };
        for url in &peers.urls {
            let fetched = async {
                let response = peers
                    .client
                    .get(format!("{url}{MEDIA_PATH}/{tenant_id}/{file}"))
                    .send()
                    .await?
                    .error_for_status()?;
                let bytes = response.bytes().await?;
                anyhow::ensure!(
                    bytes.len() as u64 == size,
                    "got {} bytes, expected {size}",
                    bytes.len()
                );
                anyhow::ensure!(
                    Self::sniff(&bytes).is_some_and(|sniffed| sniffed.ext == ext),
                    "not a {ext} picture"
                );
                self.store(tenant_id, image_id, ext, &bytes).await
            }
            .await;
            match fetched {
                Ok(()) => {
                    tracing::info!(peer = %url, tenant = %tenant_id, %file, "fetched a picture from a peer");
                    return Ok(());
                }
                Err(err) => {
                    tracing::debug!(peer = %url, tenant = %tenant_id, %file, %err, "peer has no usable copy");
                }
            }
        }
        anyhow::bail!("no peer has picture {tenant_id}/{file}")
    }

    /// Whether a request carries the cluster's secret. Without peers no
    /// request does: a single node takes no copies.
    pub fn authorizes(&self, header: Option<&[u8]>) -> bool {
        let (Some(peers), Some(header)) = (&self.peers, header) else {
            return false;
        };
        let expected = peers.authorization.as_bytes();
        // Constant time, so the answer says nothing about how close a guess was.
        header.len() == expected.len()
            && header
                .iter()
                .zip(expected)
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }

    /// Takes a copy a peer sends: checks the names cannot leave the media
    /// directory and that the bytes are the picture the name says, then
    /// stores it. `false` when it is refused.
    pub async fn accept(&self, tenant_id: &str, file: &str, bytes: &[u8]) -> anyhow::Result<bool> {
        let Some((image_id, ext)) = file.split_once('.') else {
            return Ok(false);
        };
        let valid = is_id(tenant_id)
            && is_id(image_id)
            && EXTENSIONS.contains(&ext)
            && bytes.len() as u64 <= MAX_IMAGE_BYTES
            && Self::sniff(bytes).is_some_and(|sniffed| sniffed.ext == ext);
        if !valid {
            return Ok(false);
        }
        self.store(tenant_id, image_id, ext, bytes).await?;
        Ok(true)
    }

    /// Deletes a picture known only by its id, whatever its kind.
    pub async fn remove_image(&self, tenant_id: &str, image_id: &str) {
        for ext in EXTENSIONS {
            self.remove(tenant_id, &format!("{image_id}.{ext}")).await;
        }
    }

    /// Deletes a picture. A failure is logged and otherwise ignored: the
    /// product has already stopped pointing at it, and a file nobody
    /// references is a waste of disk, not a fault.
    pub async fn remove(&self, tenant_id: &str, file: &str) {
        let path = self.dir.join(tenant_id).join(file);
        if let Err(err) = tokio::fs::remove_file(&path).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), %err, "cannot delete a picture");
        }
    }
}

/// A tenant or picture id: letters, digits, `-` and `_`, so it names one
/// entry of a directory and nothing above it.
fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The cursor identity of the subscription that keeps this node's copies in
/// step with the catalog, in each tenant's store.
pub const SUBSCRIPTION: &str = "timada-dashboard/media";

/// Follows one tenant's products: fetches each picture added that this node
/// lacks and deletes each one removed. A fetch that fails is retried, with a
/// growing delay; a picture removed since is skipped, so a node catching up
/// on an old stream does not look for files nobody keeps any more.
pub fn subscription<E: Executor>(media: Arc<Media>, tenant_id: &str) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(media)
        .data(Tenant(tenant_id.to_owned()))
        .handler(on_image_added())
        .handler(on_image_removed())
        .any_routing_key()
        // About four hours of attempts at most a minute apart: a picture is
        // on a majority of nodes before it is recorded, so a fetch only fails
        // while most of the cluster is down.
        .retry(u8::MAX)
}

#[derive(Clone)]
struct Tenant(String);

#[evento::subscription]
async fn on_image_added<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ImageAdded>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    let tenant: Tenant = context.extract();
    let still_there = product_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .is_some_and(|product| {
            product
                .images
                .iter()
                .any(|image| image.id == event.data.image_id)
        });
    if !still_there {
        return Ok(());
    }
    media
        .fetch(
            &tenant.0,
            &event.data.image_id,
            &event.data.ext,
            event.data.size,
        )
        .await
}

#[evento::subscription]
async fn on_image_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ImageRemoved>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    let tenant: Tenant = context.extract();
    media.remove_image(&tenant.0, &event.data.image_id).await;
    Ok(())
}

/// Where the pictures are served from, and the catch-all the files sit
/// under.
pub const MEDIA_PATH: &str = "/media";
pub const MEDIA_ROUTE: &str = "/media/{*file}";

/// How long a browser or a cache may keep a picture. The file at an address
/// never changes (a new picture is a new id), so: as long as it likes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// Adds that header to everything under `/media`.
pub struct MediaCache {
    path: RoutePathBuf,
}

impl MediaCache {
    pub fn new() -> Self {
        Self {
            path: MEDIA_PATH.into_path().into_owned(),
        }
    }
}

impl Layer for MediaCache {
    fn path(&self) -> Option<&RoutePath> {
        Some(&self.path)
    }

    fn handle<'cx>(&'cx self, cx: &'cx Cx, body: Body, next: Next<'cx>) -> LayerFuture<'cx> {
        Box::pin(async move {
            let mut response: Response = next.run(cx, body).await?;
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1×1 pixel of each kind, as the bytes a browser would post.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89,
    ];
    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xFF\xFF\xFF";

    #[test]
    fn knows_a_picture_from_anything_else() {
        let png = Media::sniff(PNG).expect("a PNG");
        assert_eq!(png.ext, "png");
        assert_eq!(png.mime, "image/png");
        assert_eq!((png.width, png.height), (Some(1), Some(1)));
        let gif = Media::sniff(GIF).expect("a GIF");
        assert_eq!(gif.ext, "gif");
        assert_eq!(gif.mime, "image/gif");

        assert!(Media::sniff(b"not a picture at all").is_none());
        assert!(Media::sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").is_none());
        assert!(Media::sniff(&[]).is_none());
    }

    #[test]
    fn builds_the_addresses_a_storefront_is_given() {
        let media = Media::new(
            &MediaConfig {
                dir: "data/media".into(),
                public_url: "https://shop.example/media".into(),
            },
            None,
        )
        .expect("a media store");
        assert_eq!(media.tenant_url("t-1"), "https://shop.example/media/t-1");
        assert_eq!(
            media.url("t-1", "img-1.png"),
            "https://shop.example/media/t-1/img-1.png"
        );
    }

    /// A media store in a fresh directory, with these peers.
    fn media(peers: Vec<String>, nodes: usize) -> Media {
        let dir = std::env::temp_dir().join(format!("timada-media-{}", ulid::Ulid::generate()));
        Media {
            dir,
            public_url: "https://shop.example/media".into(),
            peers: Some(Peers {
                urls: peers,
                authorization: "Bearer a-secret-shared-by-every-node-of-it".into(),
                quorum: nodes / 2,
                client: reqwest::Client::new(),
            }),
        }
    }

    /// A peer answering every request with `status` and `body`.
    async fn peer(status: u16, body: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    // Read the head, then as much body as it announces.
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let length = loop {
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
                            break end + 4 + length;
                        }
                    };
                    while request.len() < length {
                        let read = stream.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&chunk[..read]);
                    }
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                });
            }
        });
        format!("http://{addr}")
    }

    /// An address nothing listens on.
    async fn down() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    #[tokio::test]
    async fn an_upload_needs_a_majority_of_the_cluster() {
        // Three nodes: this one and one peer are a majority.
        let three = media(vec![peer(204, b"").await, down().await], 3);
        assert!(three.replicate("t1", "i1.png", PNG.to_vec()).await.is_ok());

        // This one alone is not.
        let alone = media(vec![peer(401, b"").await, down().await], 3);
        assert!(alone.replicate("t1", "i1.png", PNG.to_vec()).await.is_err());

        // Five nodes: two peers more are needed.
        let five_ok = media(
            vec![
                peer(204, b"").await,
                peer(204, b"").await,
                down().await,
                peer(500, b"").await,
            ],
            5,
        );
        assert!(
            five_ok
                .replicate("t1", "i1.png", PNG.to_vec())
                .await
                .is_ok()
        );
        let five_short = media(
            vec![
                peer(204, b"").await,
                down().await,
                down().await,
                peer(422, b"").await,
            ],
            5,
        );
        assert!(
            five_short
                .replicate("t1", "i1.png", PNG.to_vec())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_missing_picture_is_fetched_from_a_peer_that_has_it() {
        let size = PNG.len() as u64;
        let media = media(
            vec![down().await, peer(404, b"").await, peer(200, PNG).await],
            4,
        );
        media.fetch("t1", "i1", "png", size).await.expect("fetched");
        let stored = tokio::fs::read(media.dir.join("t1/i1.png")).await.unwrap();
        assert_eq!(stored, PNG);
        // Held now: nothing is asked of the peers.
        let held = Media {
            peers: None,
            ..media
        };
        held.fetch("t1", "i1", "png", size)
            .await
            .expect("already here");

        // A copy that is not what the catalog recorded is not kept.
        let wrong = super::tests::media(vec![peer(200, GIF).await], 3);
        assert!(wrong.fetch("t1", "i2", "png", size).await.is_err());
        assert!(!wrong.dir.join("t1/i2.png").exists());
    }

    #[tokio::test]
    async fn a_copy_needs_the_secret_and_a_name_inside_the_directory() {
        let media = media(vec![], 1);
        assert!(media.authorizes(Some(b"Bearer a-secret-shared-by-every-node-of-it")));
        assert!(!media.authorizes(Some(b"Bearer a-secret-shared-by-every-node-of-iX")));
        assert!(!media.authorizes(Some(b"Bearer ")));
        assert!(!media.authorizes(None));
        let single = Media {
            peers: None,
            ..media
        };
        assert!(!single.authorizes(Some(b"Bearer a-secret-shared-by-every-node-of-it")));

        let media = super::tests::media(vec![], 3);
        for (tenant, file) in [
            ("..", "i1.png"),
            ("t1", "../i1.png"),
            ("t1", "i1.exe"),
            ("t/1", "i1.png"),
        ] {
            assert!(
                !media.accept(tenant, file, PNG).await.unwrap(),
                "{tenant}/{file}"
            );
        }
        // The bytes must be what the name says.
        assert!(!media.accept("t1", "i1.gif", PNG).await.unwrap());
        assert!(media.accept("t1", "i1.png", PNG).await.unwrap());
        assert!(media.dir.join("t1/i1.png").is_file());
        media.remove_image("t1", "i1").await;
        assert!(!media.dir.join("t1/i1.png").exists());
    }
}
