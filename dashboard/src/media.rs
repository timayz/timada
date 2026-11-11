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
//! the events survive. Each node also follows the catalog's stream
//! ([`subscription`]) and fetches from a peer any picture it lacks (a node
//! that was down during an upload, or a new one), checking each copy against
//! the size and digest the event recorded, and deletes the ones taken off a
//! product.
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
use evento::subscription::{Context, SubscriptionBuilder};
use evento::{Executor, metadata::Event};
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageReader, imageops::FilterType};
use sha2::{Digest, Sha256};
use timada_catalog::aggregator::{Deleted as ProductDeleted, ImageAdded, ImageRemoved};
use timada_catalog::{MAX_IMAGE_BYTES, product_projection};
use timada_core::{MAX_PICTURE_PIXELS, Picture, RENDITION_EXT, Rendition, RenditionFile};
use timada_pages::aggregator::{
    Deleted as PageDeleted, ImageAdded as PageImageAdded, ImageRemoved as PageImageRemoved,
};
use timada_pages::page_projection;
use timada_reviews::aggregator::{
    PhotoRemoved as ReviewPhotoRemoved, Rejected as ReviewRejected, Revised as ReviewRevised,
    Submitted as ReviewSubmitted,
};
use timada_reviews::review_projection;
use timada_tenancy::aggregator::MailBrandingSet;
use timada_tenancy::tenant_projection;
use tokio::sync::Semaphore;

use topcoat::context::Cx;
use topcoat::router::{
    Body, HeaderValue, IntoPath, Layer, LayerFuture, Next, Path as RoutePath,
    PathBuf as RoutePathBuf, StatusCode, header::CACHE_CONTROL, response::Response,
};

use crate::config::MediaConfig;
use crate::peers::{self, Peers, is_id};

/// The route a node takes a copy of a picture on, below `/cluster/media`.
pub const PEER_ROUTE: &str = "/cluster/media";

/// The most a rendition may weigh. A 1200 px WebP of a photograph is a few
/// hundred kilobytes at most; a copy heavier than this is not one of ours.
pub const MAX_RENDITION_BYTES: u64 = 2 * 1024 * 1024;

/// How many uploads this node renders at once. Decoding a 25-megapixel
/// upload takes a hundred megabytes, and a merchant uploading a gallery
/// sends its pictures together.
const RENDERS_AT_ONCE: usize = 2;

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

impl Media {
    pub fn new(config: &MediaConfig, peers: Option<Arc<Peers>>) -> Self {
        Self {
            dir: config.dir.clone(),
            public_url: config.public_url.clone(),
            peers,
            renders: Semaphore::new(RENDERS_AT_ONCE),
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
                self.store(tenant_id, rendition, image_id, rendered.bytes(rendition))
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

    /// Writes one rendition. The bytes land in a temporary file first, so a
    /// reader never sees a half-written one.
    pub async fn store(
        &self,
        tenant_id: &str,
        rendition: Rendition,
        image_id: &str,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        let path = self.path_of(tenant_id, rendition, image_id)?;
        let dir = path.parent().expect("a rendition directory");
        tokio::fs::create_dir_all(dir).await?;
        let partial = path.with_extension(format!("{RENDITION_EXT}.part"));
        tokio::fs::write(&partial, bytes).await?;
        tokio::fs::rename(&partial, &path).await?;
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

    /// Makes sure this node holds every rendition of a picture the catalog
    /// references, fetching from a peer that does what it lacks. A copy
    /// fetched must be the size and have the digest the catalog recorded.
    pub async fn fetch(
        &self,
        tenant_id: &str,
        image_id: &str,
        picture: &Picture,
    ) -> anyhow::Result<()> {
        for (rendition, recorded) in picture.files() {
            self.fetch_rendition(tenant_id, image_id, rendition, recorded)
                .await?;
        }
        Ok(())
    }

    async fn fetch_rendition(
        &self,
        tenant_id: &str,
        image_id: &str,
        rendition: Rendition,
        recorded: &RenditionFile,
    ) -> anyhow::Result<()> {
        let path = self.path_of(tenant_id, rendition, image_id)?;
        let file = Picture::file(image_id, rendition);
        if tokio::fs::metadata(&path)
            .await
            .is_ok_and(|meta| meta.len() == recorded.size)
        {
            return Ok(());
        }
        let Some(peers) = &self.peers else {
            anyhow::bail!(
                "picture {tenant_id}/{file} is missing and there is no peer to fetch it from"
            );
        };
        for url in peers.urls() {
            let fetched = async {
                let response = peers
                    .client()
                    .get(format!("{url}{MEDIA_PATH}/{tenant_id}/{file}"))
                    .send()
                    .await?
                    .error_for_status()?;
                let bytes = response.bytes().await?;
                check_copy(&bytes, recorded)?;
                self.store(tenant_id, rendition, image_id, &bytes).await
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
        self.store(tenant_id, rendition, &image_id, bytes).await?;
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

/// The cursor identity of the subscription that keeps this node's copies in
/// step with the catalog and the reviews, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/media";

/// Follows every tenant's products: fetches each picture added that this node
/// lacks and deletes each one removed. A fetch that fails is retried, with a
/// growing delay; a picture removed since is skipped, so a node catching up
/// on an old stream does not look for files nobody keeps any more.
pub fn subscription<E: Executor>(media: Arc<Media>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(media)
        .handler(on_image_added())
        .handler(on_image_removed())
        .handler(on_product_deleted())
        .handler(on_review_submitted())
        .handler(on_review_rejected())
        .handler(on_review_photo_removed())
        .handler(on_review_revised())
        .handler(on_page_image_added())
        .handler(on_page_image_removed())
        .handler(on_page_deleted())
        .any_routing_key()
        // A picture is on a majority of nodes before it is recorded, so a
        // fetch only fails while most of the cluster is down; then the worker
        // ends and the node's supervisor starts it again from this event.
        .retry(5)
}

/// The cursor identity of [`logo_subscription`], in the platform store.
pub const LOGO_SUBSCRIPTION: &str = "timada-dashboard/mail-logos";

/// Follows the tenants in the platform store (the default tenant, where they
/// live: a shop's store key can write the same event in the shop's own
/// tenant, under any `aggregate_id`, and such an event is never read) and
/// fetches each mail logo this node lacks. A logo replaced since is skipped,
/// as a removed product picture is. A replaced logo's file stays on disk,
/// unreferenced.
pub fn logo_subscription<E: Executor>(media: Arc<Media>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(LOGO_SUBSCRIPTION)
        .data(media)
        .handler(on_mail_branding_set())
        .any_routing_key()
        .tenant("")
        .retry(5)
}

#[evento::subscription]
async fn on_mail_branding_set<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailBrandingSet>,
) -> anyhow::Result<()> {
    // Nothing is taken from the event but the stream it names: the branding
    // fetched is the one the tenant holds now.
    let Some(set) = &event.data.logo else {
        return Ok(());
    };
    let media: Arc<Media> = context.extract();
    let Some(logo) = tenant_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .and_then(|tenant| tenant.mail_branding)
        .and_then(|branding| branding.logo)
        .filter(|logo| logo.id == set.id)
    else {
        return Ok(());
    };
    media
        .fetch(&event.aggregate_id, &logo.id, &logo.picture)
        .await
}

#[evento::subscription]
async fn on_image_added<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ImageAdded>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    // Not the pictures of a deleted product: it still lists them, and
    // nobody keeps them any more.
    let still_there = product_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
        .filter(|product| !product.deleted)
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
        .fetch(&event.tenant, &event.data.image_id, &event.data.picture)
        .await
}

#[evento::subscription]
async fn on_image_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ImageRemoved>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    media
        .remove_image(&event.tenant, &event.data.image_id)
        .await;
    Ok(())
}

#[evento::subscription]
async fn on_product_deleted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ProductDeleted>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    // The deleted state still lists the pictures the product had.
    let Some(product) = product_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    for image in &product.images {
        media.remove_image(&event.tenant, &image.id).await;
    }
    Ok(())
}

#[evento::subscription]
async fn on_review_submitted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ReviewSubmitted>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    // Only the photos still on a review that is not rejected: the others
    // were deleted since, and nobody keeps them any more.
    let Some(review) = review_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
        .filter(|review| !review.is_rejected())
    else {
        return Ok(());
    };
    for photo in &review.photos {
        media
            .fetch(&event.tenant, &photo.id, &photo.picture)
            .await?;
    }
    Ok(())
}

#[evento::subscription]
async fn on_review_rejected<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ReviewRejected>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    let Some(review) = review_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    for photo in &review.photos {
        media.remove_image(&event.tenant, &photo.id).await;
    }
    Ok(())
}

#[evento::subscription]
async fn on_review_photo_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ReviewPhotoRemoved>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    media
        .remove_image(&event.tenant, &event.data.image_id)
        .await;
    Ok(())
}

#[evento::subscription]
async fn on_review_revised<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ReviewRevised>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    for image_id in &event.data.removed {
        media.remove_image(&event.tenant, image_id).await;
    }
    // As on a submission: only the added photos still on a review that is
    // not rejected.
    let Some(review) = review_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
        .filter(|review| !review.is_rejected())
    else {
        return Ok(());
    };
    for photo in &review.photos {
        if event.data.added.iter().any(|added| added.id == photo.id) {
            media
                .fetch(&event.tenant, &photo.id, &photo.picture)
                .await?;
        }
    }
    Ok(())
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

/// Sets that header on everything under `/media`.
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
            let cache = cache_control(response.status());
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static(cache));
            Ok(response)
        })
    }
}

#[evento::subscription]
async fn on_page_image_added<E: Executor>(
    context: &Context<'_, E>,
    event: Event<PageImageAdded>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    // Only a picture still on a page that is still there.
    let still_there = page_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
        .filter(|page| !page.deleted)
        .is_some_and(|page| {
            page.images
                .iter()
                .any(|image| image.id == event.data.image_id)
        });
    if !still_there {
        return Ok(());
    }
    media
        .fetch(&event.tenant, &event.data.image_id, &event.data.picture)
        .await
}

#[evento::subscription]
async fn on_page_image_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<PageImageRemoved>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    media
        .remove_image(&event.tenant, &event.data.image_id)
        .await;
    Ok(())
}

#[evento::subscription]
async fn on_page_deleted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<PageDeleted>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    // The deleted state still lists the pictures the page had.
    let Some(page) = page_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    for image in &page.images {
        media.remove_image(&event.tenant, &image.id).await;
    }
    Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::test_support::{png, rendered};
    use super::*;

    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xFF\xFF\xFF";

    use crate::peers::test_support::{SECRET, answering, down, peer};

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
    fn media(peers: Vec<String>, nodes: usize) -> Media {
        let dir = std::env::temp_dir().join(format!("timada-media-{}", ulid::Ulid::generate()));
        Media {
            dir,
            public_url: "https://shop.example/media".into(),
            peers: Some(Arc::new(Peers::for_test(peers, nodes))),
            renders: Semaphore::new(RENDERS_AT_ONCE),
        }
    }

    fn alone(media: Media) -> Media {
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
        media
            .fetch("t1", "i1", &upload.picture)
            .await
            .expect("fetched");
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
                    .store(tenant, Rendition::Page, image, &upload.page)
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
            .store("t1", Rendition::Listing, "i1", &upload.listing)
            .await
            .unwrap();
        media.remove_image("t1", "i1").await;
        assert!(!media.dir.join("t1/400/i1.webp").exists());
    }

    /// Tenants live in the platform's streams. A shop's store key can write a
    /// `MailBrandingSet` in the shop's own tenant under another tenant's id:
    /// nothing is fetched into that tenant's media for it.
    #[tokio::test]
    async fn a_branding_in_a_shops_tenant_fetches_nothing() {
        let upload = rendered(&png(20, 20, [1, 2, 3, 255]));
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        evento::append("victim")
            .original_version(0)
            .event(&MailBrandingSet {
                logo: Some(timada_tenancy::MailLogo {
                    id: "i1".to_owned(),
                    picture: upload.picture,
                }),
                accent: None,
                footer: None,
                set_by: "mallory".to_owned(),
            })
            .tenant("zeta")
            .commit(&store)
            .await
            .unwrap();
        let listing = upload.listing.clone();
        let has_it = answering(move |_, _| (200, listing.to_vec())).await;
        let media = Arc::new(media(vec![has_it], 1));

        logo_subscription::<evento::Turso>(media.clone())
            .ephemeral()
            .run_once(&store)
            .await
            .unwrap();
        assert!(
            tokio::fs::metadata(media.path_of("victim", Rendition::Listing, "i1").unwrap())
                .await
                .is_err()
        );
    }
}
