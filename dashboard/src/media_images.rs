//! Pictures an import brings in (from a snapshot's files, or fetched from
//! where the shop it came from serves them), rendered and kept where this
//! node serves pictures from and on a majority of the cluster, like an
//! upload.

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
    /// this machine or a private network. The check here covers literal
    /// addresses and the name's answer at the time; `client` should connect
    /// through [`crate::net::PublicResolver`] too (`net::public_client`), so
    /// the address dialled is checked as well.
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

    /// Renders the bytes into what the shop keeps, then stores them under a
    /// fresh id; `what` names them in the log. `None` when they are not a
    /// picture the shop takes.
    async fn keep(&self, bytes: bytes::Bytes, what: &str) -> anyhow::Result<Option<StoredImage>> {
        if bytes.is_empty() {
            tracing::warn!(what, "picture empty, left out");
            return Ok(None);
        }
        let rendered = match self.media.render(bytes).await? {
            Ok(rendered) => rendered,
            Err(refusal) => {
                tracing::warn!(what, ?refusal, "not a picture the shop accepts, left out");
                return Ok(None);
            }
        };
        let image_id = new_image_id();
        let picture = self
            .media
            .keep(&self.tenant_id, &image_id, rendered)
            .await?;
        Ok(Some(StoredImage { image_id, picture }))
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
                .remove_image(&self.tenant_id, &image.image_id)
                .await;
        })
    }
}
