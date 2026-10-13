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

use std::path::{Path, PathBuf};

use topcoat::context::Cx;
use topcoat::router::{
    Body, HeaderValue, IntoPath, Layer, LayerFuture, Next, Path as RoutePath,
    PathBuf as RoutePathBuf, header::CACHE_CONTROL, response::Response,
};

use crate::config::MediaConfig;

/// Where the files are and the address they are served from.
pub struct Media {
    dir: PathBuf,
    public_url: String,
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
    pub fn new(config: &MediaConfig) -> Self {
        Self {
            dir: config.dir.clone(),
            public_url: config.public_url.clone(),
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
        let media = Media::new(&MediaConfig {
            dir: "data/media".into(),
            public_url: "https://shop.example/media".into(),
        });
        assert_eq!(media.tenant_url("t-1"), "https://shop.example/media/t-1");
        assert_eq!(
            media.url("t-1", "img-1.png"),
            "https://shop.example/media/t-1/img-1.png"
        );
    }
}
