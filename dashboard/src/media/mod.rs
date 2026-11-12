//! The pictures merchants upload for their products.
//!
//! The event store records what a picture is (`timada_core::Picture`: its
//! upload's dimensions and its two renditions, each with its size and the
//! digest of its bytes); the bytes live on disk under `media.dir`, one
//! directory per tenant and one per rendition below it, named after the
//! picture's id: `<tenant>/400/<id>.webp` for listings and
//! `<tenant>/1200/<id>.webp` for the page. They are served from `/media/...`
//! by topcoat's directory route, with no authentication: an id is an
//! unguessable ULID, and a shopper's browser has no session on this node.
//!
//! The upload itself is not kept. Whoever takes one decodes it, bounds it to
//! each rendition's size and encodes it as WebP ([`Media::render`]), so what
//! every node stores is a few tens of kilobytes per picture whatever the
//! merchant uploaded, and what a listing sends a browser is the small one.
//!
//! Storing before committing and deleting after means a failure leaves an
//! unreferenced file rather than a product pointing at nothing.
//!
//! With several nodes every node keeps every picture, so one that goes down
//! takes none with it. An upload copies the renditions to the other nodes
//! and is only recorded once a majority of nodes hold them, the same
//! majority a commit needs: a picture the catalog knows survives whatever
//! the events survive. This node and each peer sync the files to the disk,
//! names and all, before they answer ([`crate::disk`]), as the store syncs
//! each write, so that holds through a power cut across the cluster too; a
//! copy a node fetches has its bytes on the disk before its name, so no file
//! is ever served empty. Each node also follows the catalog's stream
//! ([`subscription`], [`read_model`]): it records every picture's files in
//! a read model and on its own list of files to fetch, which the
//! [`Fetcher`] works off the cursor, taking from a peer what the node lacks
//! (a node that was down during an upload, or a new one) and checking each
//! copy against the size and digest the event recorded; and it deletes the
//! files of the pictures taken off a product. A miss on `/media` for a file
//! the store knows fetches it the same way before answering
//! ([`MediaCache`]).
//!
//! A shop's mail logo is kept the same way too. Its uploads are recorded in the
//! platform's stream rather than a tenant's, so [`logo_subscription`] follows
//! that one.
//!
//! The photos shoppers attach to their reviews are kept the same way, next to
//! the product pictures: a rejected review, or a photo the merchant takes
//! off one, takes its files with it.
//!
//! So are the pictures of the shop's pages (`timada_pages`): a picture taken
//! off a page, or a page deleted, takes its files with it.
//!
//! Every file name is checked before it touches the disk ([`Media::path_of`]):
//! the ids come from events, and a key with `store:write` can put anything in
//! a tenant's stream, so a name is never trusted to stay inside the tenant's
//! directory.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageReader, imageops::FilterType};
use sha2::{Digest, Sha256};
use timada_catalog::MAX_IMAGE_BYTES;
use timada_core::{
    MAX_PICTURE_PIXELS, Picture, RENDITION_EXT, RENDITION_MIME, Rendition, RenditionFile,
};
use tokio::sync::Semaphore;

use topcoat::context::Cx;
use topcoat::router::{
    Body, HeaderValue, IntoPath, Layer, LayerFuture, Next, Path as RoutePath,
    PathBuf as RoutePathBuf, StatusCode,
    header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
    response::Response,
};

use crate::config::MediaConfig;
use crate::db::Db;
use crate::disk::{self, Durability};
use crate::peers::{self, Peers, is_id};

pub mod fetcher;
pub mod quota;
pub mod read_model;
pub mod reconcile;
mod schema;

pub use fetcher::Fetcher;
pub use quota::{Quota, Room, STORAGE_FULL};
pub use read_model::{logo_subscription, subscription};
pub use reconcile::Reconcile;
pub use schema::{LOGO_SUBSCRIPTION, MIGRATIONS, NODE_MIGRATIONS, SUBSCRIPTION, TABLES};

/// The route a node takes a copy of a picture on, below `/cluster/media`.
pub const PEER_ROUTE: &str = "/cluster/media";

/// The most a rendition may weigh. A 1200 px WebP of a photograph is a few
/// hundred kilobytes at most; a copy heavier than this is not one of ours.
pub const MAX_RENDITION_BYTES: u64 = 2 * 1024 * 1024;

/// How many uploads this node renders at once. Decoding a 25-megapixel
/// upload takes a hundred megabytes, and a merchant uploading a gallery
/// sends its pictures together.
const RENDERS_AT_ONCE: usize = 2;

/// How many renditions this node asks the peers for at once: the fetcher's
/// lanes and the misses on `/media` together. A new node pulling a shop's
/// pictures takes them this many at a time.
pub const FETCHES_AT_ONCE: usize = 8;

/// The WebP quality of each rendition, out of 100: the page one is looked
/// at, the listing one is small anyway.
const PAGE_QUALITY: f32 = 82.0;
const LISTING_QUALITY: f32 = 80.0;

/// Where the files are and the address they are served from.
pub struct Media {
    dir: PathBuf,
    public_url: String,
    /// The other nodes, when there are any.
    peers: Option<Arc<Peers>>,
    renders: Semaphore,
    /// How many pictures are being asked of the peers at once.
    fetches: Semaphore,
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

/// Why an upload is not taken: what a page tells the merchant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not a JPEG, PNG, WebP or GIF, or one that cannot be decoded.
    NotAPicture,
    /// Over [`MAX_IMAGE_BYTES`].
    TooLarge,
    /// Over [`MAX_PICTURE_PIXELS`].
    TooManyPixels,
}

impl Refusal {
    /// The error code the pages word (`crate::ui::say`).
    pub fn code(self) -> &'static str {
        match self {
            Self::NotAPicture => "unsupported_image",
            Self::TooLarge => "image_too_large",
            Self::TooManyPixels => "image_too_big",
        }
    }
}

/// An upload rendered into what the shop keeps, not yet stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub picture: Picture,
    listing: Bytes,
    page: Bytes,
}

impl Rendered {
    fn bytes(&self, rendition: Rendition) -> &Bytes {
        match rendition {
            Rendition::Listing => &self.listing,
            Rendition::Page => &self.page,
        }
    }
}

/// What asking the peers for a rendition came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    /// This node had it already.
    Held,
    /// Taken from a peer and stored.
    Fetched,
    /// Every peer that could be asked said it has no copy (or holds one that
    /// is not the file recorded), and none was out of reach: the file is
    /// gone for good, and so is asking again.
    Gone,
    /// Some peer could not be asked (down, or not serving), so a copy may
    /// still be had later; why, for the log.
    Unreachable(String),
}

impl Media {
    pub fn new(config: &MediaConfig, peers: Option<Arc<Peers>>) -> Self {
        Self {
            dir: config.dir.clone(),
            public_url: config.public_url.clone(),
            peers,
            renders: Semaphore::new(RENDERS_AT_ONCE),
            fetches: Semaphore::new(FETCHES_AT_ONCE),
        }
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

    /// The address of one rendition of a picture.
    pub fn url(&self, tenant_id: &str, image_id: &str, rendition: Rendition) -> String {
        Picture::url(&self.tenant_url(tenant_id), image_id, rendition)
    }

    /// Where a rendition's file is, once its names are checked: the tenant
    /// and the picture are ids ([`is_id`]: one directory entry each, nothing
    /// above). Everything that touches the disk goes through here, because
    /// the names come from events and a key with `store:write` writes what
    /// it likes there.
    pub fn path_of(
        &self,
        tenant_id: &str,
        rendition: Rendition,
        image_id: &str,
    ) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(is_id(tenant_id), "{tenant_id:?} is not a tenant id");
        anyhow::ensure!(is_id(image_id), "{image_id:?} is not a picture id");
        Ok(self
            .dir
            .join(tenant_id)
            .join(rendition.dir())
            .join(format!("{image_id}.{RENDITION_EXT}")))
    }

    /// The rendition and picture a copy's address names (`<dir>/<id>.webp`
    /// below the tenant), checked the same way.
    fn parse_file(dir: &str, file: &str) -> anyhow::Result<(Rendition, String)> {
        let Some(rendition) = Rendition::from_dir(dir) else {
            anyhow::bail!("{dir:?} is not a rendition");
        };
        let Some(image_id) = file.strip_suffix(&format!(".{RENDITION_EXT}")) else {
            anyhow::bail!("{file:?} is not a rendition's file name");
        };
        anyhow::ensure!(is_id(image_id), "{image_id:?} is not a picture id");
        Ok((rendition, image_id.to_owned()))
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

    /// Renders an upload into the two renditions the shop keeps, off the
    /// async threads and a couple at a time. `Err` is a refusal to tell the
    /// merchant; the outer error is this node's.
    pub async fn render(&self, bytes: Bytes) -> anyhow::Result<Result<Rendered, Refusal>> {
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Ok(Err(Refusal::TooLarge));
        }
        let _slot = self.renders.acquire().await?;
        Ok(tokio::task::spawn_blocking(move || render(&bytes)).await?)
    }

    /// Writes a rendered upload to this node's disk and copies it to a
    /// majority of the cluster, like the event that will record it: a node
    /// going down takes no picture with it. Fails, with the files deleted
    /// again, when this node cannot write or too few nodes took the copies;
    /// the picture must then not be recorded. What it returns is what the
    /// event records.
    pub async fn keep(
        &self,
        tenant_id: &str,
        image_id: &str,
        rendered: Rendered,
    ) -> anyhow::Result<Picture> {
        let kept = async {
            for rendition in Rendition::ALL {
                let bytes = rendered.bytes(rendition);
                self.store(tenant_id, rendition, image_id, bytes, Durability::Synced)
                    .await?;
            }
            self.replicate(tenant_id, image_id, &rendered).await
        }
        .await;
        if let Err(err) = kept {
            self.remove_image(tenant_id, image_id).await;
            return Err(err);
        }
        Ok(rendered.picture)
    }

    /// Writes one rendition. The bytes are on the disk before the file takes
    /// its name ([`disk::write_partial`]), so neither a reader nor a power
    /// cut ever finds a half-written or empty one; with
    /// [`Durability::Synced`] the name is too when this returns.
    pub async fn store(
        &self,
        tenant_id: &str,
        rendition: Rendition,
        image_id: &str,
        bytes: &[u8],
        durability: Durability,
    ) -> anyhow::Result<()> {
        let path = self.path_of(tenant_id, rendition, image_id)?;
        let bytes = bytes.to_vec();
        tokio::task::spawn_blocking(move || {
            let partial = disk::write_partial(&path, &bytes)?;
            std::fs::rename(&partial, &path).inspect_err(|_| {
                let _ = std::fs::remove_file(&partial);
            })?;
            let dir = path.parent().expect("a rendition directory");
            match durability {
                Durability::Synced => disk::sync_dir(dir),
                Durability::Cached => Ok(()),
            }
        })
        .await??;
        Ok(())
    }

    /// Copies a picture's renditions to the other nodes, returning once a
    /// majority of the cluster (this node included) holds both. The copies
    /// to the remaining nodes carry on in the background.
    async fn replicate(
        &self,
        tenant_id: &str,
        image_id: &str,
        rendered: &Rendered,
    ) -> anyhow::Result<()> {
        let Some(peers) = &self.peers else {
            return Ok(());
        };
        let copies = Rendition::ALL.map(|rendition| {
            let path = format!(
                "{PEER_ROUTE}/{tenant_id}/{}",
                Picture::file(image_id, rendition)
            );
            let bytes = rendered.bytes(rendition).clone();
            async move { peers.put_to_quorum(&path, bytes, "picture").await }
        });
        let [listing, page] = copies;
        let (listing, page) = tokio::join!(listing, page);
        listing.and(page)
    }

    /// Makes sure this node holds one rendition of a picture, asking the
    /// peers in turn for what it lacks; a copy is kept only if it is the
    /// size and has the digest recorded. A bounded number run at once. The
    /// error is this node's own (a name that is not ours, a disk that will
    /// not take the file); what the peers said is the [`Fetch`].
    pub async fn fetch_rendition(
        &self,
        tenant_id: &str,
        image_id: &str,
        rendition: Rendition,
        recorded: &RenditionFile,
    ) -> anyhow::Result<Fetch> {
        let path = self.path_of(tenant_id, rendition, image_id)?;
        let file = Picture::file(image_id, rendition);
        if tokio::fs::metadata(&path)
            .await
            .is_ok_and(|meta| meta.len() == recorded.size)
        {
            return Ok(Fetch::Held);
        }
        let Some(peers) = &self.peers else {
            // Alone, there is nobody to ask: the file is gone for good.
            return Ok(Fetch::Gone);
        };
        let _slot = self.fetches.acquire().await?;
        let mut unreachable = None;
        for url in peers.urls() {
            let response = peers
                .client()
                .get(format!("{url}{MEDIA_PATH}/{tenant_id}/{file}"))
                .send()
                .await;
            let response = match response {
                Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
                    tracing::debug!(peer = %url, tenant = %tenant_id, %file, "peer has no copy");
                    continue;
                }
                Ok(response) if response.status().is_success() => response,
                Ok(response) => {
                    // The peer is up but not serving: not an answer about
                    // the file.
                    tracing::debug!(peer = %url, tenant = %tenant_id, %file, status = %response.status(), "peer did not answer for the picture");
                    unreachable
                        .get_or_insert_with(|| format!("{url} answered {}", response.status()));
                    continue;
                }
                Err(err) => {
                    tracing::debug!(peer = %url, tenant = %tenant_id, %file, %err, "peer not reached");
                    unreachable.get_or_insert_with(|| format!("{url}: {err}"));
                    continue;
                }
            };
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(err) => {
                    unreachable.get_or_insert_with(|| format!("{url}: {err}"));
                    continue;
                }
            };
            if let Err(err) = check_copy(&bytes, recorded) {
                // A copy that is not the file recorded is no copy; the
                // peer's disk may hold an older upload under the id.
                tracing::warn!(peer = %url, tenant = %tenant_id, %file, %err, "peer's copy is not the picture recorded");
                continue;
            }
            self.store(tenant_id, rendition, image_id, &bytes, Durability::Cached)
                .await?;
            tracing::info!(peer = %url, tenant = %tenant_id, %file, "fetched a picture from a peer");
            return Ok(Fetch::Fetched);
        }
        Ok(match unreachable {
            Some(reason) => Fetch::Unreachable(reason),
            None => Fetch::Gone,
        })
    }

    /// Whether a request carries the cluster's secret. Without peers no
    /// request does: a single node takes no copies.
    pub fn authorizes(&self, header: Option<&[u8]>) -> bool {
        peers::authorizes(self.peers.as_deref(), header)
    }

    /// Takes a copy a peer sends, at `<dir>/<file>` below the tenant: checks
    /// the names cannot leave the media directory ([`Self::path_of`]) and
    /// that the bytes are a rendition (WebP, no heavier than one gets), then
    /// stores it. `false` when it is refused.
    pub async fn accept(
        &self,
        tenant_id: &str,
        dir: &str,
        file: &str,
        bytes: &[u8],
    ) -> anyhow::Result<bool> {
        let Ok((rendition, image_id)) = Self::parse_file(dir, file) else {
            return Ok(false);
        };
        let valid = self.path_of(tenant_id, rendition, &image_id).is_ok()
            && bytes.len() as u64 <= MAX_RENDITION_BYTES
            && is_rendition(bytes);
        if !valid {
            return Ok(false);
        }
        self.store(tenant_id, rendition, &image_id, bytes, Durability::Synced)
            .await?;
        Ok(true)
    }

    /// Deletes every rendition of a picture. A failure is logged and
    /// otherwise ignored: the product has already stopped pointing at it,
    /// and a file nobody references is a waste of disk, not a fault. A name
    /// that is not a picture of ours is logged and touches nothing.
    pub async fn remove_image(&self, tenant_id: &str, image_id: &str) {
        for rendition in Rendition::ALL {
            let path = match self.path_of(tenant_id, rendition, image_id) {
                Ok(path) => path,
                Err(err) => {
                    tracing::warn!(tenant = %tenant_id, image = %image_id, %err, "not a picture of ours: not deleted");
                    return;
                }
            };
            if let Err(err) = tokio::fs::remove_file(&path).await
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(path = %path.display(), %err, "cannot delete a picture");
            }
        }
    }
}

/// Whether a copy is the file the catalog recorded: its size and digest.
fn check_copy(bytes: &[u8], recorded: &RenditionFile) -> anyhow::Result<()> {
    anyhow::ensure!(
        bytes.len() as u64 == recorded.size,
        "got {} bytes, expected {}",
        bytes.len(),
        recorded.size
    );
    anyhow::ensure!(
        sha256(bytes) == recorded.sha256,
        "the bytes are not the ones recorded"
    );
    Ok(())
}

/// Whether the bytes are a WebP picture, as every rendition is.
fn is_rendition(bytes: &[u8]) -> bool {
    Media::sniff(bytes).is_some_and(|sniffed| sniffed.ext == RENDITION_EXT)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Renders an upload: decodes it (JPEG, PNG, WebP or GIF, the first frame
/// of an animation, turned the way its EXIF orientation says), bounds it
/// to the page rendition's side and then to the listing's, never enlarging,
/// and encodes each as WebP. Blocking: a second or so for a 25-megapixel
/// photograph.
fn render(bytes: &[u8]) -> Result<Rendered, Refusal> {
    // The header first: an upload with too many pixels is refused before a
    // byte of it is decoded.
    let sniffed = Media::sniff(bytes).ok_or(Refusal::NotAPicture)?;
    let (Some(width), Some(height)) = (sniffed.width, sniffed.height) else {
        return Err(Refusal::NotAPicture);
    };
    if u64::from(width) * u64::from(height) > MAX_PICTURE_PIXELS || width == 0 || height == 0 {
        return Err(Refusal::TooManyPixels);
    }
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| Refusal::NotAPicture)?
        .into_decoder()
        .map_err(|_| Refusal::NotAPicture)?;
    // What the header said is what gets decoded, and no more.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(MAX_PICTURE_PIXELS * 8);
    decoder
        .set_limits(limits)
        .map_err(|_| Refusal::TooManyPixels)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut upload = DynamicImage::from_decoder(decoder).map_err(|_| Refusal::NotAPicture)?;
    upload.apply_orientation(orientation);
    let (width, height) = upload.dimensions();
    if width == 0 || height == 0 {
        return Err(Refusal::NotAPicture);
    }

    let page_image = fit(&upload, Rendition::Page);
    let listing_image = fit(&page_image, Rendition::Listing);
    let (page, page_file) = encode(&page_image, PAGE_QUALITY)?;
    let (listing, listing_file) = encode(&listing_image, LISTING_QUALITY)?;
    Ok(Rendered {
        picture: Picture {
            width,
            height,
            listing: listing_file,
            page: page_file,
        },
        listing,
        page,
    })
}

/// The picture within the rendition's side, shrunk if it is larger and
/// left alone otherwise.
fn fit(image: &DynamicImage, rendition: Rendition) -> DynamicImage {
    let side = rendition.max_side();
    if image.width() <= side && image.height() <= side {
        return image.clone();
    }
    image.resize(side, side, FilterType::Lanczos3)
}

/// The picture as WebP, with what the store records of the file.
fn encode(image: &DynamicImage, quality: f32) -> Result<(Bytes, RenditionFile), Refusal> {
    let (width, height) = image.dimensions();
    let encoded = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        webp::Encoder::from_rgba(rgba.as_raw(), width, height).encode(quality)
    } else {
        let rgb = image.to_rgb8();
        webp::Encoder::from_rgb(rgb.as_raw(), width, height).encode(quality)
    };
    let bytes = Bytes::copy_from_slice(&encoded);
    if bytes.is_empty() || bytes.len() as u64 > MAX_RENDITION_BYTES {
        return Err(Refusal::NotAPicture);
    }
    let file = RenditionFile {
        width,
        height,
        size: bytes.len() as u64,
        sha256: sha256(&bytes),
    };
    Ok((bytes, file))
}

/// Where the pictures are served from, and the catch-all the files sit
/// under.
pub const MEDIA_PATH: &str = "/media";
pub const MEDIA_ROUTE: &str = "/media/{*file}";

/// How long a browser or a cache may keep a picture. The file at an address
/// never changes (a new picture is a new id), so: as long as it likes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// What a cache may keep of a picture this node could not serve: nothing.
/// The file may be on its way (a node that is catching up fetches its copies
/// from the others), and a miss kept for a year would hide it once it lands.
const NO_STORE: &str = "no-store";

/// The `Cache-Control` of an answer under `/media`, by its status.
fn cache_control(status: StatusCode) -> &'static str {
    if status.is_success() {
        IMMUTABLE
    } else {
        NO_STORE
    }
}

/// Sets that header on everything under `/media`, and answers a miss for a
/// file the store knows by fetching it from a peer first: a node that has
/// not got a picture yet (down during the upload, or still catching up)
/// serves it all the same, and keeps it. A miss for a file the store does
/// not know is a 404 at once, with no peer asked: an address nobody was
/// given costs the peers nothing.
pub struct MediaCache {
    path: RoutePathBuf,
    media: Arc<Media>,
    db: Db,
}

impl MediaCache {
    pub fn new(media: Arc<Media>, db: Db) -> Self {
        Self {
            path: MEDIA_PATH.into_path().into_owned(),
            media,
            db,
        }
    }

    /// The tenant, rendition and picture a request path below `/media`
    /// names, when it names one: `/media/<tenant>/<400|1200>/<id>.webp`.
    fn parse_path(path: &str) -> Option<(String, Rendition, String)> {
        let rest = path.strip_prefix(MEDIA_PATH)?.strip_prefix('/')?;
        let mut parts = rest.split('/');
        let (tenant_id, dir, file) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || !is_id(tenant_id) {
            return None;
        }
        let (rendition, image_id) = Media::parse_file(dir, file).ok()?;
        Some((tenant_id.to_owned(), rendition, image_id))
    }

    /// The file for a miss, fetched from a peer when the store knows it and
    /// a peer has it; `None` when it stays a miss.
    async fn fetch_missing(&self, path: &str) -> Option<Bytes> {
        let (tenant_id, rendition, image_id) = Self::parse_path(path)?;
        let recorded = match read_model::recorded(&self.db, &tenant_id, &image_id, rendition).await
        {
            Ok(recorded) => recorded?,
            Err(err) => {
                tracing::warn!(%path, error = %format!("{err:#}"), "cannot look a missing picture up");
                return None;
            }
        };
        match self
            .media
            .fetch_rendition(&tenant_id, &image_id, rendition, &recorded)
            .await
        {
            Ok(Fetch::Held | Fetch::Fetched) => {}
            Ok(Fetch::Gone) => return None,
            Ok(Fetch::Unreachable(reason)) => {
                tracing::debug!(%path, %reason, "a missing picture could not be fetched for the request");
                return None;
            }
            Err(err) => {
                tracing::warn!(%path, error = %format!("{err:#}"), "a missing picture could not be fetched for the request");
                return None;
            }
        }
        let file = self.media.path_of(&tenant_id, rendition, &image_id).ok()?;
        tokio::fs::read(&file).await.ok().map(Bytes::from)
    }
}

impl Layer for MediaCache {
    fn path(&self) -> Option<&RoutePath> {
        Some(&self.path)
    }

    fn handle<'cx>(&'cx self, cx: &'cx Cx, body: Body, next: Next<'cx>) -> LayerFuture<'cx> {
        Box::pin(async move {
            let path = topcoat::router::request::original_uri(cx).path().to_owned();
            let mut response: Response = next.run(cx, body).await?;
            if response.status() == StatusCode::NOT_FOUND
                && let Some(bytes) = self.fetch_missing(&path).await
            {
                response = Response::new(Body::from(bytes.clone()));
                let headers = response.headers_mut();
                headers.insert(CONTENT_TYPE, HeaderValue::from_static(RENDITION_MIME));
                headers.insert(
                    CONTENT_LENGTH,
                    HeaderValue::from_str(&bytes.len().to_string()).expect("digits"),
                );
            }
            let cache = cache_control(response.status());
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static(cache));
            Ok(response)
        })
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A PNG of `width` × `height` in one colour, as a browser would post it.
    pub fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        let mut out = Cursor::new(Vec::new());
        image
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("a PNG");
        out.into_inner()
    }

    /// A rendered upload: what [`Media::render`] makes of `png`.
    pub fn rendered(bytes: &[u8]) -> Rendered {
        render(bytes).expect("a picture")
    }

    impl Media {
        /// Every rendition of a picture, fetched or held; an error for one
        /// that could not be had, for whatever reason.
        pub async fn fetch(
            &self,
            tenant_id: &str,
            image_id: &str,
            picture: &Picture,
        ) -> anyhow::Result<()> {
            for (rendition, recorded) in picture.files() {
                match self
                    .fetch_rendition(tenant_id, image_id, rendition, recorded)
                    .await?
                {
                    Fetch::Held | Fetch::Fetched => {}
                    other => anyhow::bail!("{rendition:?} not fetched: {other:?}"),
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::test_support::{png, rendered};
    use super::*;
    use crate::disk::test_support::Syncs;

    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xFF\xFF\xFF";

    use crate::peers::test_support::{SECRET, answering, down, peer};

    /// A miss names a file the store knows or it stays a miss: only then is
    /// a peer asked, and the file fetched is served and kept.
    #[tokio::test]
    async fn a_miss_for_a_known_file_fetches_it_from_a_peer() {
        use super::fetcher::tests::fetcher_with;
        use super::read_model::Source;

        assert_eq!(
            MediaCache::parse_path("/media/t1/400/i1.webp"),
            Some(("t1".to_owned(), Rendition::Listing, "i1".to_owned()))
        );
        for path in [
            "/media/t1/i1.webp",
            "/media/t1/800/i1.webp",
            "/media/t1/400/i1.png",
            "/media/../400/i1.webp",
            "/media/t1/400/i1.webp/x",
            "/other/t1/400/i1.webp",
        ] {
            assert_eq!(MediaCache::parse_path(path), None, "{path}");
        }

        let upload = rendered(&png(900, 600, [7, 7, 7, 255]));
        let (listing, page) = (upload.listing.clone(), upload.page.clone());
        let has_it = answering(move |head, _| {
            let body = if head.contains("/400/") {
                listing.to_vec()
            } else {
                page.to_vec()
            };
            (200, body)
        })
        .await;
        let (_store, fetcher) = fetcher_with(vec![has_it], 3).await;
        let cache = MediaCache::new(fetcher.media.clone(), fetcher.db.clone());
        // Unknown to the store: no peer asked, a miss.
        assert!(
            cache
                .fetch_missing("/media/acme/400/i1.webp")
                .await
                .is_none()
        );
        fetcher
            .record("acme", "i1", &upload.picture, Source::Product, "mat", 1)
            .await
            .unwrap();
        let served = cache
            .fetch_missing("/media/acme/400/i1.webp")
            .await
            .expect("fetched");
        assert_eq!(served, upload.listing);
        assert!(
            fetcher
                .media
                .path_of("acme", Rendition::Listing, "i1")
                .unwrap()
                .is_file(),
            "kept"
        );
        assert!(
            !fetcher
                .media
                .path_of("acme", Rendition::Page, "i1")
                .unwrap()
                .exists(),
            "only what was asked for"
        );
    }

    /// A picture served is kept for a year; a miss is not kept at all, so a
    /// cache in front never hides a picture this node copies later.
    #[test]
    fn a_miss_is_never_cached() {
        assert_eq!(cache_control(StatusCode::OK), IMMUTABLE);
        assert_eq!(cache_control(StatusCode::NOT_FOUND), NO_STORE);
        assert_eq!(cache_control(StatusCode::INTERNAL_SERVER_ERROR), NO_STORE);
    }

    #[test]
    fn knows_a_picture_from_anything_else() {
        let png = Media::sniff(&png(3, 2, [0, 0, 255, 255])).expect("a PNG");
        assert_eq!(png.ext, "png");
        assert_eq!(png.mime, "image/png");
        assert_eq!((png.width, png.height), (Some(3), Some(2)));
        let gif = Media::sniff(GIF).expect("a GIF");
        assert_eq!(gif.ext, "gif");
        assert_eq!(gif.mime, "image/gif");

        assert!(Media::sniff(b"not a picture at all").is_none());
        assert!(Media::sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").is_none());
        assert!(Media::sniff(&[]).is_none());
    }

    /// An upload becomes two WebP files, each within its side and never
    /// enlarged, and the record says what the upload and the files were.
    #[test]
    fn an_upload_is_rendered_in_two_sizes() {
        let wide = rendered(&png(3000, 1000, [200, 30, 30, 255]));
        assert_eq!((wide.picture.width, wide.picture.height), (3000, 1000));
        assert_eq!(
            (wide.picture.page.width, wide.picture.page.height),
            (1200, 400)
        );
        assert_eq!(
            (wide.picture.listing.width, wide.picture.listing.height),
            (400, 133)
        );
        for (rendition, file) in wide.picture.files() {
            let bytes = wide.bytes(rendition);
            assert_eq!(bytes.len() as u64, file.size);
            assert_eq!(sha256(bytes), file.sha256);
            assert!(is_rendition(bytes), "{rendition:?} is WebP");
            let decoded = image::load_from_memory(bytes).expect("decodes");
            assert_eq!(decoded.dimensions(), (file.width, file.height));
        }
        assert!(wide.picture.is_valid());

        let small = rendered(&png(50, 80, [0, 0, 0, 128]));
        assert_eq!(
            (small.picture.page.width, small.picture.page.height),
            (50, 80)
        );
        assert_eq!(
            (small.picture.listing.width, small.picture.listing.height),
            (50, 80),
            "never enlarged"
        );
        // Its transparency survives.
        let decoded = image::load_from_memory(&small.page).unwrap();
        assert!(decoded.color().has_alpha());
    }

    /// What is not a picture, or more picture than the shop takes, is
    /// refused with the word for it, before it is decoded.
    #[test]
    fn an_upload_that_is_not_a_picture_is_refused() {
        assert_eq!(render(b"not a picture"), Err(Refusal::NotAPicture));
        assert_eq!(render(&[]), Err(Refusal::NotAPicture));
        // A PNG header claiming 6000 × 6000 pixels, and nothing behind it.
        let mut huge = png(1, 1, [0; 4]);
        huge[16..20].copy_from_slice(&6000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&6000u32.to_be_bytes());
        assert_eq!(render(&huge), Err(Refusal::TooManyPixels));
        // A header that lies about a size within the cap is found out.
        let mut lying = png(1, 1, [0; 4]);
        lying[16..20].copy_from_slice(&600u32.to_be_bytes());
        assert!(render(&lying).is_err());
        assert_eq!(Refusal::TooLarge.code(), "image_too_large");
        assert_eq!(Refusal::TooManyPixels.code(), "image_too_big");
    }

    #[test]
    fn builds_the_addresses_a_storefront_is_given() {
        let media = Media::new(
            &MediaConfig {
                dir: "data/media".into(),
                public_url: "https://shop.example/media".into(),
                tenant_quota_bytes: 0,
                reconcile_check_secs: 86_400,
            },
            None,
        );
        assert_eq!(media.tenant_url("t-1"), "https://shop.example/media/t-1");
        assert_eq!(
            media.url("t-1", "img-1", Rendition::Listing),
            "https://shop.example/media/t-1/400/img-1.webp"
        );
        assert_eq!(
            media.url("t-1", "img-1", Rendition::Page),
            "https://shop.example/media/t-1/1200/img-1.webp"
        );
    }

    /// A media store in a fresh directory, with these peers.
    pub(crate) fn media(peers: Vec<String>, nodes: usize) -> Media {
        let dir = std::env::temp_dir().join(format!("timada-media-{}", ulid::Ulid::generate()));
        Media {
            dir,
            public_url: "https://shop.example/media".into(),
            peers: Some(Arc::new(Peers::for_test(peers, nodes))),
            renders: Semaphore::new(RENDERS_AT_ONCE),
            fetches: Semaphore::new(FETCHES_AT_ONCE),
        }
    }

    pub(crate) fn alone(media: Media) -> Media {
        Media {
            peers: None,
            ..media
        }
    }

    #[tokio::test]
    async fn an_upload_is_kept_once_a_majority_of_the_cluster_has_it() {
        let upload = rendered(&png(900, 600, [10, 200, 10, 255]));
        let three = media(vec![peer(204, b"").await, down().await], 3);
        let picture = three.keep("t1", "i1", upload.clone()).await.expect("kept");
        assert_eq!(picture, upload.picture);
        for rendition in Rendition::ALL {
            let path = three.path_of("t1", rendition, "i1").unwrap();
            assert_eq!(
                tokio::fs::read(&path).await.unwrap().len() as u64,
                picture.rendition(rendition).size
            );
        }

        // Short of a majority, nothing is kept here either.
        let short = media(vec![peer(401, b"").await, down().await], 3);
        assert!(short.keep("t1", "i1", upload.clone()).await.is_err());
        assert!(
            !short.dir.join("t1").exists()
                || !short.path_of("t1", Rendition::Page, "i1").unwrap().exists()
        );

        let single = alone(media(vec![], 1));
        assert!(single.keep("t1", "i1", upload).await.is_ok());
    }

    /// An upload's files are on the disk, names and all, before the upload
    /// is recorded, and so is the copy a peer takes of one.
    #[tokio::test]
    async fn a_picture_counted_on_is_synced_name_and_all() {
        let upload = rendered(&png(900, 600, [10, 200, 10, 255]));
        let media = media(vec![peer(204, b"").await], 3);
        // The directories a first picture creates, out of the way.
        media.keep("t1", "i0", upload.clone()).await.unwrap();
        let mut syncs = Syncs::below(&media.dir);

        media.keep("t1", "i1", upload.clone()).await.unwrap();
        assert_eq!(
            syncs.take(),
            ["t1/1200/", "t1/1200/i1.webp", "t1/400/", "t1/400/i1.webp"]
        );
        assert!(
            media
                .accept("t1", "400", "i2.webp", &upload.listing)
                .await
                .unwrap()
        );
        assert_eq!(syncs.take(), ["t1/400/", "t1/400/i2.webp"]);
    }

    /// A node lacking a picture takes it from the first peer whose copy is
    /// the one recorded, rendition by rendition.
    #[tokio::test]
    async fn a_missing_picture_is_fetched_from_a_peer_that_has_it() {
        let upload = rendered(&png(900, 600, [10, 10, 200, 255]));
        let (listing, page) = (upload.listing.clone(), upload.page.clone());
        let has_it = answering(move |head, _| {
            let body = if head.contains("/400/") {
                listing.to_vec()
            } else {
                page.to_vec()
            };
            (200, body)
        })
        .await;
        let media = media(vec![down().await, peer(404, b"").await, has_it], 4);
        let mut syncs = Syncs::below(&media.dir);
        media
            .fetch("t1", "i1", &upload.picture)
            .await
            .expect("fetched");
        // The bytes, not the names (those synced are the directories
        // created, each into the one above it).
        assert_eq!(
            syncs.take(),
            ["/", "t1/", "t1/", "t1/1200/i1.webp", "t1/400/i1.webp"]
        );
        for (rendition, file) in upload.picture.files() {
            let stored = tokio::fs::read(media.path_of("t1", rendition, "i1").unwrap())
                .await
                .unwrap();
            assert_eq!(stored, upload.bytes(rendition).to_vec());
            assert_eq!(stored.len() as u64, file.size);
        }
        // Held now: nothing is asked of the peers.
        alone(media)
            .fetch("t1", "i1", &upload.picture)
            .await
            .expect("already here");

        // A copy that is not what the catalog recorded is not kept: the
        // right size with other bytes, or another picture altogether.
        let other = rendered(&png(900, 600, [200, 10, 10, 255]));
        let wrong = super::tests::media(vec![peer(200, b"").await], 3);
        assert!(wrong.fetch("t1", "i2", &upload.picture).await.is_err());
        let swapped = {
            let page = other.page.clone();
            answering(move |_, _| (200, page.to_vec())).await
        };
        let swapped = super::tests::media(vec![swapped], 3);
        assert!(swapped.fetch("t1", "i2", &upload.picture).await.is_err());
        assert!(!swapped.dir.join("t1").exists());
    }

    #[tokio::test]
    async fn a_copy_needs_the_secret_and_a_name_inside_the_directory() {
        let media = media(vec![], 1);
        assert!(media.authorizes(Some(SECRET.as_bytes())));
        assert!(!media.authorizes(None));
        let single = alone(media);
        assert!(!single.authorizes(Some(SECRET.as_bytes())));

        let upload = rendered(&png(20, 20, [1, 2, 3, 255]));
        let webp = upload.listing.to_vec();
        let media = super::tests::media(vec![], 3);
        for (tenant, dir, file) in [
            ("..", "400", "i1.webp"),
            ("t1", "..", "i1.webp"),
            ("t1", "400", "../i1.webp"),
            ("t1", "400", "i1.exe"),
            ("t1", "400", "i1.png"),
            ("t1", "800", "i1.webp"),
            ("t/1", "400", "i1.webp"),
        ] {
            assert!(
                !media.accept(tenant, dir, file, &webp).await.unwrap(),
                "{tenant}/{dir}/{file}"
            );
        }
        // The bytes must be a rendition: WebP, and no heavier than one gets.
        assert!(
            !media
                .accept("t1", "400", "i1.webp", &png(2, 2, [0; 4]))
                .await
                .unwrap()
        );
        assert!(
            !media
                .accept(
                    "t1",
                    "400",
                    "i1.webp",
                    &vec![0; MAX_RENDITION_BYTES as usize + 1]
                )
                .await
                .unwrap()
        );
        assert!(media.accept("t1", "400", "i1.webp", &webp).await.unwrap());
        assert!(media.dir.join("t1/400/i1.webp").is_file());
        media.remove_image("t1", "i1").await;
        assert!(!media.dir.join("t1/400/i1.webp").exists());
    }

    /// The names come from events, which a key with `store:write` fills as
    /// it likes: none of them gets a file written, read or deleted outside
    /// the tenant's directory.
    #[tokio::test]
    async fn a_name_from_the_store_cannot_leave_the_tenant_directory() {
        let upload = rendered(&png(20, 20, [1, 2, 3, 255]));
        let media = media(vec![peer(200, b"").await], 3);
        let bad_names = [
            ("..", "i1"),
            ("t1", "../../etc/passwd"),
            ("t1", "i1/../../x"),
            ("t/1", "i1"),
            ("", "i1"),
            ("t1", ""),
            ("t1", "i1.webp"),
        ];
        for (tenant, image) in bad_names {
            assert!(
                media.path_of(tenant, Rendition::Page, image).is_err(),
                "{tenant}/{image}"
            );
            assert!(
                media
                    .store(
                        tenant,
                        Rendition::Page,
                        image,
                        &upload.page,
                        Durability::Cached
                    )
                    .await
                    .is_err(),
                "stored {tenant}/{image}"
            );
            assert!(
                media.fetch(tenant, image, &upload.picture).await.is_err(),
                "fetched {tenant}/{image}"
            );
        }
        assert_eq!(
            media.path_of("t1", Rendition::Listing, "i1").unwrap(),
            media.dir.join("t1/400/i1.webp")
        );

        // A file above the tenant's directory survives every way of asking
        // for its deletion.
        tokio::fs::create_dir_all(media.dir.join("t1/400"))
            .await
            .unwrap();
        let above = media.dir.join("i1.webp");
        tokio::fs::write(&above, &upload.listing).await.unwrap();
        media.remove_image("t1", "..").await;
        media.remove_image("t1", "../i1").await;
        media.remove_image("..", "i1").await;
        assert!(above.is_file());
        // While a picture of ours goes.
        media
            .store(
                "t1",
                Rendition::Listing,
                "i1",
                &upload.listing,
                Durability::Cached,
            )
            .await
            .unwrap();
        media.remove_image("t1", "i1").await;
        assert!(!media.dir.join("t1/400/i1.webp").exists());
    }
}
