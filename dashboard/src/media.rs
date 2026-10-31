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
//! the ids and extensions come from events, and a key with `store:write` can
//! put anything in a tenant's stream, so a name is never trusted to stay
//! inside the tenant's directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use evento::subscription::{Context, SubscriptionBuilder};
use evento::{Executor, metadata::Event};
use timada_catalog::aggregator::{ImageAdded, ImageRemoved};
use timada_catalog::{MAX_IMAGE_BYTES, product_projection};
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

use topcoat::context::Cx;
use topcoat::router::{
    Body, HeaderValue, IntoPath, Layer, LayerFuture, Next, Path as RoutePath,
    PathBuf as RoutePathBuf, header::CACHE_CONTROL, response::Response,
};

use crate::config::MediaConfig;
use crate::peers::{self, Peers, is_id};

/// The kinds of picture the shop takes, by extension.
const EXTENSIONS: [&str; 4] = ["jpg", "png", "webp", "gif"];

/// The route a node takes a copy of a picture on, below `/cluster/media`.
pub const PEER_ROUTE: &str = "/cluster/media";

/// Where the files are and the address they are served from.
pub struct Media {
    dir: PathBuf,
    public_url: String,
    /// The other nodes, when there are any.
    peers: Option<Arc<Peers>>,
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
    pub fn new(config: &MediaConfig, peers: Option<Arc<Peers>>) -> Self {
        Self {
            dir: config.dir.clone(),
            public_url: config.public_url.clone(),
            peers,
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

    /// The address of one picture.
    pub fn url(&self, tenant_id: &str, file: &str) -> String {
        format!("{}/{tenant_id}/{file}", self.public_url)
    }

    /// Where a picture's file is, once its names are checked: the tenant and
    /// the picture are ids ([`is_id`]: one directory entry each, nothing
    /// above) and the extension one of the kinds the shop takes. Everything
    /// that touches the disk goes through here, because the names come from
    /// events and a key with `store:write` writes what it likes there.
    pub fn path_of(&self, tenant_id: &str, image_id: &str, ext: &str) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(is_id(tenant_id), "{tenant_id:?} is not a tenant id");
        anyhow::ensure!(is_id(image_id), "{image_id:?} is not a picture id");
        anyhow::ensure!(
            EXTENSIONS.contains(&ext),
            "{ext:?} is not a kind of picture the shop takes"
        );
        Ok(self.dir.join(tenant_id).join(format!("{image_id}.{ext}")))
    }

    /// Where the file named `file` (`<id>.<ext>`) is, checked the same way.
    fn path_of_file(&self, tenant_id: &str, file: &str) -> anyhow::Result<PathBuf> {
        let Some((image_id, ext)) = file.split_once('.') else {
            anyhow::bail!("{file:?} is not a picture file name");
        };
        self.path_of(tenant_id, image_id, ext)
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
        let path = self.path_of(tenant_id, image_id, ext)?;
        let dir = path.parent().expect("a tenant directory");
        tokio::fs::create_dir_all(dir).await?;
        let partial = path.with_extension(format!("{ext}.part"));
        tokio::fs::write(&partial, bytes).await?;
        tokio::fs::rename(&partial, &path).await?;
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
        peers
            .put_to_quorum(
                &format!("{PEER_ROUTE}/{tenant_id}/{file}"),
                bytes.into(),
                "picture",
            )
            .await
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
        let path = self.path_of(tenant_id, image_id, ext)?;
        let file = format!("{image_id}.{ext}");
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
        for url in peers.urls() {
            let fetched = async {
                let response = peers
                    .client()
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
        peers::authorizes(self.peers.as_deref(), header)
    }

    /// Takes a copy a peer sends: checks the names cannot leave the media
    /// directory ([`Self::path_of`]) and that the bytes are the picture the
    /// name says, then stores it. `false` when it is refused.
    pub async fn accept(&self, tenant_id: &str, file: &str, bytes: &[u8]) -> anyhow::Result<bool> {
        let Some((image_id, ext)) = file.split_once('.') else {
            return Ok(false);
        };
        let valid = self.path_of(tenant_id, image_id, ext).is_ok()
            && bytes.len() as u64 <= MAX_IMAGE_BYTES
            && Self::sniff(bytes).is_some_and(|sniffed| sniffed.ext == ext);
        if !valid {
            return Ok(false);
        }
        self.store(tenant_id, image_id, ext, bytes).await?;
        Ok(true)
    }

    /// Deletes a picture known only by its id, whatever its kind. A name
    /// that is not an id names nothing of ours: logged, nothing touched.
    pub async fn remove_image(&self, tenant_id: &str, image_id: &str) {
        if !is_id(tenant_id) || !is_id(image_id) {
            tracing::warn!(tenant = %tenant_id, image = %image_id, "not a picture of ours: not deleted");
            return;
        }
        for ext in EXTENSIONS {
            self.remove(tenant_id, &format!("{image_id}.{ext}")).await;
        }
    }

    /// Deletes a picture (`file` is `<id>.<ext>`). A failure is logged and
    /// otherwise ignored: the product has already stopped pointing at it,
    /// and a file nobody references is a waste of disk, not a fault. A name
    /// that is not a picture of ours is logged and touches nothing.
    pub async fn remove(&self, tenant_id: &str, file: &str) {
        let path = match self.path_of_file(tenant_id, file) {
            Ok(path) => path,
            Err(err) => {
                tracing::warn!(tenant = %tenant_id, %file, %err, "not a picture of ours: not deleted");
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
        .handler(on_review_submitted())
        .handler(on_review_rejected())
        .handler(on_review_photo_removed())
        .handler(on_review_revised())
        .handler(on_page_image_added())
        .handler(on_page_image_removed())
        .handler(on_page_deleted())
        .any_routing_key()
        // About four hours of attempts at most a minute apart: a picture is
        // on a majority of nodes before it is recorded, so a fetch only fails
        // while most of the cluster is down.
        .retry(u8::MAX)
}

/// The cursor identity of [`logo_subscription`], in the platform store.
pub const LOGO_SUBSCRIPTION: &str = "timada-dashboard/mail-logos";

/// Follows the tenants in the platform store and fetches each mail logo
/// this node lacks. A logo replaced since is skipped, as a removed product
/// picture is. A replaced logo's file stays on disk, unreferenced.
pub fn logo_subscription<E: Executor>(media: Arc<Media>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(LOGO_SUBSCRIPTION)
        .data(media)
        .handler(on_mail_branding_set())
        .any_routing_key()
        .retry(u8::MAX)
}

#[evento::subscription]
async fn on_mail_branding_set<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MailBrandingSet>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    let Some(file) = event.data.logo_file.as_deref() else {
        return Ok(());
    };
    let still_there = tenant_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
        .and_then(|tenant| tenant.mail_branding)
        .is_some_and(|branding| branding.logo_file.as_deref() == Some(file));
    let Some((id, ext)) = file.split_once('.').filter(|_| still_there) else {
        return Ok(());
    };
    media
        .fetch(&event.aggregate_id, id, ext, event.data.logo_size)
        .await
}

#[evento::subscription]
async fn on_image_added<E: Executor>(
    context: &Context<'_, E>,
    event: Event<ImageAdded>,
) -> anyhow::Result<()> {
    let media: Arc<Media> = context.extract();
    let still_there = product_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
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
            &event.tenant,
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
    media
        .remove_image(&event.tenant, &event.data.image_id)
        .await;
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
            .fetch(&event.tenant, &photo.id, &photo.ext, photo.size)
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
                .fetch(&event.tenant, &photo.id, &photo.ext, photo.size)
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
        .fetch(
            &event.tenant,
            &event.data.image_id,
            &event.data.ext,
            event.data.size,
        )
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
mod tests {
    use super::*;

    /// A 1×1 pixel of each kind, as the bytes a browser would post.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89,
    ];
    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xFF\xFF\xFF";

    use crate::peers::test_support::{SECRET, down, peer};

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
        );
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
            peers: Some(Arc::new(Peers::for_test(peers, nodes))),
        }
    }

    #[tokio::test]
    async fn an_upload_is_copied_to_a_majority_of_the_cluster() {
        let three = media(vec![peer(204, b"").await, down().await], 3);
        assert!(three.replicate("t1", "i1.png", PNG.to_vec()).await.is_ok());
        let alone = media(vec![peer(401, b"").await, down().await], 3);
        assert!(alone.replicate("t1", "i1.png", PNG.to_vec()).await.is_err());
        let single = Media {
            peers: None,
            ..alone
        };
        assert!(single.replicate("t1", "i1.png", PNG.to_vec()).await.is_ok());
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
        assert!(media.authorizes(Some(SECRET.as_bytes())));
        assert!(!media.authorizes(None));
        let single = Media {
            peers: None,
            ..media
        };
        assert!(!single.authorizes(Some(SECRET.as_bytes())));

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

    /// The names come from events, which a key with `store:write` fills as
    /// it likes: none of them gets a file written, read or deleted outside
    /// the tenant's directory.
    #[tokio::test]
    async fn a_name_from_the_store_cannot_leave_the_tenant_directory() {
        let media = media(vec![peer(200, PNG).await], 3);
        let bad_names = [
            ("..", "i1", "png"),
            ("t1", "../../etc/passwd", "png"),
            ("t1", "i1", "png/../../x"),
            ("t1", "i1", "exe"),
            ("t/1", "i1", "png"),
            ("", "i1", "png"),
            ("t1", "", "png"),
            ("t1", "i1", ""),
        ];
        for (tenant, image, ext) in bad_names {
            assert!(
                media.path_of(tenant, image, ext).is_err(),
                "{tenant}/{image}.{ext}"
            );
            assert!(
                media.store(tenant, image, ext, PNG).await.is_err(),
                "stored {tenant}/{image}.{ext}"
            );
            assert!(
                media
                    .fetch(tenant, image, ext, PNG.len() as u64)
                    .await
                    .is_err(),
                "fetched {tenant}/{image}.{ext}"
            );
        }
        assert_eq!(
            media.path_of("t1", "i1", "png").unwrap(),
            media.dir.join("t1/i1.png")
        );

        // A file above the tenant's directory survives every way of asking
        // for its deletion.
        tokio::fs::create_dir_all(media.dir.join("t1"))
            .await
            .unwrap();
        let above = media.dir.join("i1.png");
        tokio::fs::write(&above, PNG).await.unwrap();
        media.remove("t1", "../i1.png").await;
        media.remove("..", "i1.png").await;
        media.remove_image("t1", "..").await;
        media.remove_image("t1", "../i1").await;
        assert!(above.is_file());
        // While a picture of ours goes.
        media.store("t1", "i1", "png", PNG).await.unwrap();
        media.remove("t1", "i1.png").await;
        assert!(!media.dir.join("t1/i1.png").exists());
    }
}
