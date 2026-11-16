//! The blog's write side: an article's write model and the commands that
//! change it. An article is written, published, moved and deleted as a page
//! is; it also carries a summary, a cover and the day it came out.

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};
use timada_core::{FormerSlug, leave_slug};

use crate::aggregator::article::{
    Claimed, CoverSet, Created, Dated, Deleted, Edited, ImageAdded, ImageRemoved, Published,
    Released, Renamed, SeoSet, Unpublished,
};
use crate::error::PageError;
use crate::value_object::{
    MAX_IMAGES_PER_PAGE, article_slug, article_slug_id, new_article_id, normalize_body,
    normalize_seo, normalize_slug, normalize_summary, normalize_title,
};

use super::PageImage;

/// The write model of an article, and what a storefront shows of it.
#[evento::projection(id = id, name = "timada-pages/ArticleState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ArticleState {
    pub id: String,
    pub slug: String,
    /// The slugs it went by before a rename, oldest first: an old address
    /// is answered with a redirect to the new one.
    pub former_slugs: Vec<FormerSlug>,
    pub title: String,
    /// A sentence or two for the blog's list; may be empty.
    pub summary: String,
    /// Markdown.
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
    pub published: bool,
    /// The day it says it came out (Unix seconds): the first time it was
    /// published, unless the merchant gave another. 0 while it never was.
    pub published_at: u64,
    pub images: Vec<PageImage>,
    /// The picture, one of `images`, the blog's list shows.
    pub cover: Option<String>,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
    pub deleted: bool,
}

impl ArticleState {
    /// Whether the article still goes by `slug`: not deleted, and that is
    /// its slug.
    pub fn holds_slug(&self, slug: &str) -> bool {
        !self.deleted && self.slug == slug
    }

    /// The title search engines show: the article's own unless one was set.
    pub fn seo_title(&self) -> &str {
        if self.meta_title.is_empty() {
            &self.title
        } else {
            &self.meta_title
        }
    }

    /// The cover's picture, while the article has one.
    pub fn cover_image(&self) -> Option<&PageImage> {
        let cover = self.cover.as_deref()?;
        self.images.iter().find(|image| image.id == cover)
    }
}

pub fn article_projection<E: Executor>() -> Projection<E, ArticleState> {
    Projection::new::<crate::aggregator::article::Article>()
        .handler(on_created())
        .handler(on_edited())
        .handler(on_seo_set())
        .handler(on_renamed())
        .handler(on_published())
        .handler(on_unpublished())
        .handler(on_dated())
        .handler(on_image_added())
        .handler(on_image_removed())
        .handler(on_cover_set())
        .handler(on_deleted())
        .strict()
}

#[evento::handler]
async fn on_created(event: Event<Created>, article: &mut ArticleState) -> anyhow::Result<()> {
    article.id = event.aggregate_id.to_owned();
    article.slug = event.data.slug.to_owned();
    article.title = event.data.title.to_owned();
    article.summary = event.data.summary.to_owned();
    article.body = event.data.body.to_owned();
    article.created_at = event.timestamp;
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_edited(event: Event<Edited>, article: &mut ArticleState) -> anyhow::Result<()> {
    article.title = event.data.title.to_owned();
    article.summary = event.data.summary.to_owned();
    article.body = event.data.body.to_owned();
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_seo_set(event: Event<SeoSet>, article: &mut ArticleState) -> anyhow::Result<()> {
    article.meta_title = event.data.meta_title.to_owned();
    article.meta_description = event.data.meta_description.to_owned();
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_renamed(event: Event<Renamed>, article: &mut ArticleState) -> anyhow::Result<()> {
    leave_slug(
        &mut article.former_slugs,
        &article.slug,
        &event.data.slug,
        event.timestamp,
    );
    article.slug = event.data.slug.to_owned();
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_published(event: Event<Published>, article: &mut ArticleState) -> anyhow::Result<()> {
    article.published = true;
    if article.published_at == 0 {
        article.published_at = event.timestamp;
    }
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_unpublished(
    event: Event<Unpublished>,
    article: &mut ArticleState,
) -> anyhow::Result<()> {
    article.published = false;
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_dated(event: Event<Dated>, article: &mut ArticleState) -> anyhow::Result<()> {
    article.published_at = event.data.published_at;
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_image_added(
    event: Event<ImageAdded>,
    article: &mut ArticleState,
) -> anyhow::Result<()> {
    let data = &event.data;
    article.images.push(PageImage {
        id: data.image_id.to_owned(),
        picture: data.picture,
    });
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_image_removed(
    event: Event<ImageRemoved>,
    article: &mut ArticleState,
) -> anyhow::Result<()> {
    article
        .images
        .retain(|image| image.id != event.data.image_id);
    if article.cover.as_deref() == Some(event.data.image_id.as_str()) {
        article.cover = None;
    }
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_cover_set(event: Event<CoverSet>, article: &mut ArticleState) -> anyhow::Result<()> {
    article.cover = event.data.image_id.to_owned();
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_deleted(event: Event<Deleted>, article: &mut ArticleState) -> anyhow::Result<()> {
    article.deleted = true;
    article.published = false;
    article.updated_at = event.timestamp;
    Ok(())
}

#[evento::projection(id = id, name = "timada-pages/ArticleSlugState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ArticleSlugState {
    pub id: String,
    pub article_id: String,
    /// The version of the article the last claim was made at.
    pub article_version: u16,
    /// Unix seconds of the last claim.
    pub claimed_at: u64,
    pub released: bool,
}

pub fn article_slug_projection<E: Executor>() -> Projection<E, ArticleSlugState> {
    Projection::new::<crate::aggregator::article::ArticleSlug>()
        .handler(on_slug_claimed())
        .handler(on_slug_released())
        .strict()
}

#[evento::handler]
async fn on_slug_claimed(
    event: Event<Claimed>,
    index: &mut ArticleSlugState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.article_id = event.data.article_id.to_owned();
    index.article_version = event.data.article_version;
    index.claimed_at = event.timestamp;
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_slug_released(
    _event: Event<Released>,
    index: &mut ArticleSlugState,
) -> anyhow::Result<()> {
    index.released = true;
    Ok(())
}

impl<E: Executor> super::Command<E> {
    /// An article that is not deleted.
    pub async fn load_article(&self, article_id: &str) -> anyhow::Result<Option<ArticleState>> {
        Ok(self
            .load_article_any(article_id)
            .await?
            .filter(|article| !article.deleted))
    }

    pub(crate) async fn load_article_any(
        &self,
        article_id: &str,
    ) -> anyhow::Result<Option<ArticleState>> {
        article_projection()
            .load(article_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The article a slug names, published or not. An article moved to
    /// another slug is not returned, even while the old one waits to be
    /// released.
    pub async fn load_article_by_slug(&self, slug: &str) -> anyhow::Result<Option<ArticleState>> {
        let Some(index) = self.load_article_slug_index(slug).await? else {
            return Ok(None);
        };
        Ok(self
            .load_article(&index.article_id)
            .await?
            .filter(|article| article.holds_slug(slug)))
    }

    /// The live index entry of a slug (released ones are `None`).
    pub async fn load_article_slug_index(
        &self,
        slug: &str,
    ) -> anyhow::Result<Option<ArticleSlugState>> {
        Ok(self
            .load_article_slug_index_any(slug)
            .await?
            .filter(|index| !index.released))
    }

    pub(crate) async fn load_article_slug_index_any(
        &self,
        slug: &str,
    ) -> anyhow::Result<Option<ArticleSlugState>> {
        article_slug_projection()
            .load(article_slug_id(slug))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}

/// An article to write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateArticle {
    /// Empty derives it from the title.
    pub slug: String,
    pub title: String,
    /// A sentence or two; may be empty.
    pub summary: String,
    /// Markdown.
    pub body: String,
}

/// A picture the host already keeps, in the renditions `picture`
/// describes, to add to an article.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddArticleImage {
    pub image_id: String,
    pub picture: timada_core::Picture,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Writes a draft under a fresh id; its slug must be free. A creation
    /// that fails keeps the claim, as its write may land yet (the store
    /// could not tell): a claim whose article never got written is taken over
    /// once it is no longer fresh.
    pub async fn create_article(
        &self,
        actor: &str,
        cmd: CreateArticle,
        routing_key: Option<String>,
    ) -> Result<String, PageError> {
        let title = normalize_title(&cmd.title)?;
        let summary = normalize_summary(&cmd.summary)?;
        let body = normalize_body(&cmd.body)?;
        let slug = if cmd.slug.trim().is_empty() {
            article_slug(&title)
        } else {
            normalize_slug(&cmd.slug)?
        };
        let id = new_article_id();
        self.claim_article_slug(&slug, &id, 0, routing_key.clone())
            .await?;
        evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                slug,
                title,
                summary,
                body,
                created_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(id)
    }

    /// Rewrites the title, the summary and the body; the same text is a
    /// no-op.
    pub async fn edit_article(
        &self,
        actor: &str,
        article_id: &str,
        title: &str,
        summary: &str,
        body: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        let title = normalize_title(title)?;
        let summary = normalize_summary(summary)?;
        let body = normalize_body(body)?;
        if article.title == title && article.summary == summary && article.body == body {
            return Ok(());
        }
        self.write_article(
            &article,
            &Edited {
                title,
                summary,
                body,
                edited_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Sets what search engines show; the same is a no-op.
    pub async fn set_article_seo(
        &self,
        actor: &str,
        article_id: &str,
        meta_title: &str,
        meta_description: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        let (meta_title, meta_description) = normalize_seo(meta_title, meta_description)?;
        if article.meta_title == meta_title && article.meta_description == meta_description {
            return Ok(());
        }
        self.write_article(
            &article,
            &SeoSet {
                meta_title,
                meta_description,
                set_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Moves the article to another slug, which must be free; the old one is
    /// free to any claim once the article has moved, and the release
    /// subscription frees its entry.
    pub async fn rename_article(
        &self,
        actor: &str,
        article_id: &str,
        slug: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        let slug = normalize_slug(slug)?;
        if article.slug == slug {
            return Ok(());
        }
        let version = article.aggregate_version()?;
        self.claim_article_slug(&slug, &article.id, version, routing_key.clone())
            .await?;
        let renamed = self
            .write_article(
                &article,
                &Renamed {
                    slug: slug.clone(),
                    renamed_by: actor.to_owned(),
                },
                routing_key.clone(),
            )
            .await;
        if let Err(err) = renamed {
            let _ = self
                .release_article_slug_claim(&slug, &article.id, version, routing_key)
                .await;
            return Err(err);
        }
        Ok(())
    }

    /// Shows it on the blog, dated now unless it has a date already;
    /// already published is a no-op.
    pub async fn publish_article(
        &self,
        actor: &str,
        article_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        if article.published {
            return Ok(());
        }
        self.write_article(
            &article,
            &Published {
                published_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Takes it off the storefront; a draft already is a no-op.
    pub async fn unpublish_article(
        &self,
        actor: &str,
        article_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        if !article.published {
            return Ok(());
        }
        self.write_article(
            &article,
            &Unpublished {
                unpublished_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Dates the article: the day the blog says it came out and sorts it
    /// by, today or before. The same day is a no-op.
    pub async fn date_article(
        &self,
        actor: &str,
        article_id: &str,
        published_at: u64,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        if published_at == 0 || published_at > timada_core::unix_now() {
            return Err(PageError::InvalidArticleDate);
        }
        if article.published_at == published_at {
            return Ok(());
        }
        self.write_article(
            &article,
            &Dated {
                published_at,
                dated_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Picks the cover among the article's pictures, or leaves it without
    /// one (`None`). The same is a no-op.
    pub async fn set_article_cover(
        &self,
        actor: &str,
        article_id: &str,
        image_id: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        if article.cover.as_deref() == image_id {
            return Ok(());
        }
        if let Some(image_id) = image_id
            && !article.images.iter().any(|image| image.id == image_id)
        {
            return Err(PageError::ImageNotFound);
        }
        self.write_article(
            &article,
            &CoverSet {
                image_id: image_id.map(str::to_owned),
                set_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Records a picture the host already keeps (on a majority of nodes,
    /// with several). The same id again is a no-op.
    pub async fn add_article_image(
        &self,
        actor: &str,
        article_id: &str,
        cmd: AddArticleImage,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        if article.images.iter().any(|image| image.id == cmd.image_id) {
            return Ok(());
        }
        if article.images.len() >= MAX_IMAGES_PER_PAGE {
            return Err(PageError::TooManyImages);
        }
        if !cmd.picture.is_valid() {
            return Err(PageError::UnsupportedImage);
        }
        self.write_article(
            &article,
            &ImageAdded {
                image_id: cmd.image_id,
                picture: cmd.picture,
                added_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Takes a picture off the article, and the cover with it when it was
    /// this one; the host deletes its file after.
    pub async fn remove_article_image(
        &self,
        actor: &str,
        article_id: &str,
        image_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let article = self.require_article(article_id).await?;
        if !article.images.iter().any(|image| image.id == image_id) {
            return Err(PageError::ImageNotFound);
        }
        self.write_article(
            &article,
            &ImageRemoved {
                image_id: image_id.to_owned(),
                removed_by: actor.to_owned(),
            },
            routing_key,
        )
        .await
    }

    /// Deletes the article for good. Its slug is free to any claim at once, and
    /// the release subscription frees its entry. Returns what it was, so the
    /// host can delete its pictures' files.
    pub async fn delete_article(
        &self,
        actor: &str,
        article_id: &str,
        routing_key: Option<String>,
    ) -> Result<ArticleState, PageError> {
        let article = self.require_article(article_id).await?;
        self.write_article(
            &article,
            &Deleted {
                deleted_by: actor.to_owned(),
            },
            routing_key,
        )
        .await?;
        Ok(article)
    }
}

impl<E: Executor> super::Command<E> {
    async fn require_article(&self, article_id: &str) -> Result<ArticleState, PageError> {
        self.load_article(article_id)
            .await?
            .ok_or(PageError::ArticleNotFound)
    }

    async fn write_article<D>(
        &self,
        article: &ArticleState,
        event: &D,
        routing_key: Option<String>,
    ) -> Result<(), PageError>
    where
        D: evento::AggregateEvent + bitcode::Encode,
    {
        article
            .write()?
            .routing_key_opt(routing_key)
            .event(event)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
