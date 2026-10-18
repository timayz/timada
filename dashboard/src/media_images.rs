//! Pictures an import brings in (from a snapshot's files, or fetched from
//! where the shop it came from serves them), kept where this node serves
//! pictures from and on a majority of the cluster, like an upload.

use std::sync::Arc;

use timada_catalog::{MAX_IMAGE_BYTES, new_image_id};
use timada_catalog_import::{BoxFuture, Images, StoredImage};

use crate::media::Media;

pub struct MediaImages {
    client: reqwest::Client,
    media: Arc<Media>,
    tenant_id: String,
    /// Fetches only from public hosts: the addresses come from outside
    /// (a merchant, a supplier), not from the operator.
    public_only: bool,
}

impl MediaImages {
    /// Fetches with `client`; `public_only` refuses a picture whose host is
    /// this machine or a private network.
    pub fn new(
        client: reqwest::Client,
        media: Arc<Media>,
        tenant_id: impl Into<String>,
        public_only: bool,
    ) -> Self {
        Self {
            client,
            media,
            tenant_id: tenant_id.into(),
            public_only,
        }
    }

    /// Checks the bytes are a picture the shop takes, then stores them
    /// under a fresh id; `what` names them in the log.
    async fn keep(&self, bytes: bytes::Bytes, what: &str) -> anyhow::Result<Option<StoredImage>> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_BYTES {
            tracing::warn!(
                what,
                size = bytes.len(),
                "picture empty or too large, left out"
            );
            return Ok(None);
        }
        let Some(sniffed) = Media::sniff(&bytes) else {
            tracing::warn!(what, "not a picture the shop accepts, left out");
            return Ok(None);
        };
        let image_id = new_image_id();
        let file = format!("{image_id}.{}", sniffed.ext);
        self.media
            .store(&self.tenant_id, &image_id, sniffed.ext, &bytes)
            .await?;
        if let Err(err) = self
            .media
            .replicate(&self.tenant_id, &file, bytes.to_vec())
            .await
        {
            self.media.remove(&self.tenant_id, &file).await;
            return Err(err);
        }
        Ok(Some(StoredImage {
            image_id,
            ext: sniffed.ext.to_owned(),
            size: bytes.len() as u64,
            width: sniffed.width,
            height: sniffed.height,
        }))
    }
}

impl Images for MediaImages {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, anyhow::Result<Option<StoredImage>>> {
        Box::pin(async move {
            if self.public_only {
                crate::net::check_public_url(url).await?;
            }
            let response = self.client.get(url).send().await?.error_for_status()?;
            if response
                .content_length()
                .is_some_and(|length| length > MAX_IMAGE_BYTES)
            {
                tracing::warn!(url, "picture too large, left out");
                return Ok(None);
            }
            let bytes = response.bytes().await?;
            self.keep(bytes, url).await
        })
    }

    fn store<'a>(&'a self, bytes: Vec<u8>) -> BoxFuture<'a, anyhow::Result<Option<StoredImage>>> {
        Box::pin(async move { self.keep(bytes::Bytes::from(bytes), "snapshot").await })
    }

    fn discard<'a>(&'a self, image: &'a StoredImage) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.media
                .remove(
                    &self.tenant_id,
                    &format!("{}.{}", image.image_id, image.ext),
                )
                .await;
        })
    }
}
