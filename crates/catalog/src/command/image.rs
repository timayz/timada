//! The pictures of a product. The crate records what a picture is and where
//! it sits in the gallery; the bytes are the host's business, and so is
//! deciding that a file really is the picture it claims to be.

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{ImageAdded, ImageAltChanged, ImageRemoved, ImagesReordered};
use crate::error::CatalogError;
use crate::value_object::{MAX_IMAGE_BYTES, MAX_IMAGES_PER_PRODUCT, image_mime, normalize_alt};

/// A picture to add: what the host made of the uploaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddImage {
    /// A fresh unguessable id; the stored file is named after it.
    pub image_id: String,
    /// Lowercase, no dot, one of [`IMAGE_FORMATS`](crate::IMAGE_FORMATS).
    pub ext: String,
    /// In bytes.
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt: String,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records one more picture, at the end of the gallery.
    pub async fn add_image(
        &self,
        actor: &str,
        product_id: &str,
        cmd: AddImage,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let ext = cmd.ext.trim().to_ascii_lowercase();
        let mime = image_mime(&ext).ok_or(CatalogError::InvalidImage)?;
        if cmd.image_id.trim().is_empty() || cmd.size == 0 || cmd.size > MAX_IMAGE_BYTES {
            return Err(CatalogError::InvalidImage);
        }
        let alt = normalize_alt(&cmd.alt)?;
        let product = self.require_editable(product_id).await?;
        if product.images.len() >= MAX_IMAGES_PER_PRODUCT {
            return Err(CatalogError::TooManyImages);
        }
        if product.images.iter().any(|image| image.id == cmd.image_id) {
            return Err(CatalogError::InvalidImage);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&ImageAdded {
                image_id: cmd.image_id,
                ext,
                mime: mime.to_owned(),
                size: cmd.size,
                width: cmd.width,
                height: cmd.height,
                alt,
                added_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Changes what a picture is said to show; the same text is a no-op.
    pub async fn change_image_alt(
        &self,
        actor: &str,
        product_id: &str,
        image_id: &str,
        alt: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let alt = normalize_alt(alt)?;
        let product = self.require_editable(product_id).await?;
        let image = product
            .images
            .iter()
            .find(|image| image.id == image_id)
            .ok_or(CatalogError::ImageNotFound)?;
        if image.alt == alt {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&ImageAltChanged {
                image_id: image_id.to_owned(),
                alt,
                changed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Puts the gallery in the given order, which must list every picture of
    /// the product exactly once. The same order is a no-op.
    pub async fn reorder_images(
        &self,
        actor: &str,
        product_id: &str,
        image_ids: Vec<String>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        let current: Vec<&str> = product
            .images
            .iter()
            .map(|image| image.id.as_str())
            .collect();
        if image_ids.len() != current.len() {
            return Err(CatalogError::InvalidImageOrder);
        }
        let mut sorted = image_ids.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != image_ids.len()
            || !image_ids.iter().all(|id| current.contains(&id.as_str()))
        {
            return Err(CatalogError::InvalidImageOrder);
        }
        if current == image_ids {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&ImagesReordered {
                image_ids,
                reordered_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Takes a picture out of the gallery. One that is not there is a no-op,
    /// so removing twice is safe; the host deletes the file afterwards.
    pub async fn remove_image(
        &self,
        actor: &str,
        product_id: &str,
        image_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if !product.images.iter().any(|image| image.id == image_id) {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&ImageRemoved {
                image_id: image_id.to_owned(),
                removed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
