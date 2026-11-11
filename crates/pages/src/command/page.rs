use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{
    Created, Deleted, Edited, ImageAdded, ImageRemoved, Published, Renamed, SeoSet, Unpublished,
};
use crate::error::PageError;
use crate::value_object::{
    MAX_IMAGES_PER_PAGE, new_page_id, normalize_body, normalize_seo, normalize_slug,
    normalize_title, slugify,
};

use super::PageState;

/// A page to write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreatePage {
    /// Empty derives it from the title.
    pub slug: String,
    pub title: String,
    /// Markdown.
    pub body: String,
}

/// A picture the host already keeps, in the renditions `picture`
/// describes, to add to a page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddPageImage {
    pub image_id: String,
    pub picture: timada_core::Picture,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Writes a draft under a fresh id; its slug must be free. A creation
    /// that fails keeps the claim, as its write may land yet (the store
    /// could not tell): a claim whose page never got written is taken over
    /// once it is no longer fresh.
    pub async fn create_page(
        &self,
        actor: &str,
        cmd: CreatePage,
        routing_key: Option<String>,
    ) -> Result<String, PageError> {
        let title = normalize_title(&cmd.title)?;
        let body = normalize_body(&cmd.body)?;
        let slug = if cmd.slug.trim().is_empty() {
            slugify(&title)
        } else {
            normalize_slug(&cmd.slug)?
        };
        let id = new_page_id();
        self.claim_page_slug(&slug, &id, 0, routing_key.clone())
            .await?;
        evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                slug,
                title,
                body,
                created_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(id)
    }

    /// Rewrites the title and the body; the same text is a no-op.
    pub async fn edit_page(
        &self,
        actor: &str,
        page_id: &str,
        title: &str,
        body: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let page = self.require_page(page_id).await?;
        let title = normalize_title(title)?;
        let body = normalize_body(body)?;
        if page.title == title && page.body == body {
            return Ok(());
        }
        self.write_page(
            &page,
            &Edited {
                title,
                body,
                edited_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Sets what search engines show; the same is a no-op.
    pub async fn set_page_seo(
        &self,
        actor: &str,
        page_id: &str,
        meta_title: &str,
        meta_description: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let page = self.require_page(page_id).await?;
        let (meta_title, meta_description) = normalize_seo(meta_title, meta_description)?;
        if page.meta_title == meta_title && page.meta_description == meta_description {
            return Ok(());
        }
        self.write_page(
            &page,
            &SeoSet {
                meta_title,
                meta_description,
                set_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Moves the page to another slug, which must be free; the old one is
    /// free to any claim once the page has moved, and the release
    /// subscription frees its entry.
    pub async fn rename_page(
        &self,
        actor: &str,
        page_id: &str,
        slug: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let page = self.require_page(page_id).await?;
        let slug = normalize_slug(slug)?;
        if page.slug == slug {
            return Ok(());
        }
        let version = page.aggregate_version()?;
        self.claim_page_slug(&slug, &page.id, version, routing_key.clone())
            .await?;
        let renamed = self
            .write_page(
                &page,
                &Renamed {
                    slug: slug.clone(),
                    renamed_by: actor.to_owned(),
                },
                routing_key.clone(),
            )
            .await;
        if let Err(err) = renamed {
            let _ = self
                .release_page_slug_claim(&slug, &page.id, version, routing_key)
                .await;
            return Err(err);
        }
        Ok(())
    }

    /// Shows it on the storefront; already published is a no-op.
    pub async fn publish_page(
        &self,
        actor: &str,
        page_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let page = self.require_page(page_id).await?;
        if page.published {
            return Ok(());
        }
        self.write_page(
            &page,
            &Published {
                published_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Takes it off the storefront; a draft already is a no-op.
    pub async fn unpublish_page(
        &self,
        actor: &str,
        page_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let page = self.require_page(page_id).await?;
        if !page.published {
            return Ok(());
        }
        self.write_page(
            &page,
            &Unpublished {
                unpublished_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Records a picture the host already keeps (on a majority of nodes,
    /// with several). The same id again is a no-op.
    pub async fn add_page_image(
        &self,
        actor: &str,
        page_id: &str,
        cmd: AddPageImage,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let page = self.require_page(page_id).await?;
        if page.images.iter().any(|image| image.id == cmd.image_id) {
            return Ok(());
        }
        if page.images.len() >= MAX_IMAGES_PER_PAGE {
            return Err(PageError::TooManyImages);
        }
        if !cmd.picture.is_valid() {
            return Err(PageError::UnsupportedImage);
        }
        self.write_page(
            &page,
            &ImageAdded {
                image_id: cmd.image_id,
                picture: cmd.picture,
                added_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Takes a picture off the page; the host deletes its file after.
    pub async fn remove_page_image(
        &self,
        actor: &str,
        page_id: &str,
        image_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let page = self.require_page(page_id).await?;
        if !page.images.iter().any(|image| image.id == image_id) {
            return Err(PageError::ImageNotFound);
        }
        self.write_page(
            &page,
            &ImageRemoved {
                image_id: image_id.to_owned(),
                removed_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Deletes the page for good. Its slug is free to any claim at once, and
    /// the release subscription frees its entry. Returns what it was, so the
    /// host can delete its pictures' files.
    pub async fn delete_page(
        &self,
        actor: &str,
        page_id: &str,
        routing_key: Option<String>,
    ) -> Result<PageState, PageError> {
        let page = self.require_page(page_id).await?;
        self.write_page(
            &page,
            &Deleted {
                deleted_by: actor.to_owned(),
            },
            routing_key,
        )
        .await?;
        Ok(page)
    }
}

impl<E: Executor> super::Command<E> {
    async fn require_page(&self, page_id: &str) -> Result<PageState, PageError> {
        self.load_page(page_id).await?.ok_or(PageError::NotFound)
    }

    async fn write_page<D>(
        &self,
        page: &PageState,
        event: &D,
        routing_key: Option<String>,
    ) -> Result<(), PageError>
    where
        D: evento::AggregateEvent + bitcode::Encode,
    {
        page.write()?
            .routing_key_opt(routing_key)
            .event(event)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
