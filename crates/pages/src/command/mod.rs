//! Write side: the command gateway and the strict write models it loads.
//!
//! The write models fold *all* events of a stream (`strict()`), so the
//! version they observe, and `write()` sends as `original_version`, never
//! goes stale. They are not snapshotted: a page is edited a few dozen times
//! at most.

mod page;
mod slug_index;

use std::ops::Deref;

pub use page::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::{
    Claimed, Created, Deleted, Edited, HiddenFromFooter, ImageAdded, ImageRemoved, Published,
    Released, Renamed, SeoSet, ShownInFooter, Unpublished,
};
use crate::value_object::page_slug_id;

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes, loads or reads is
    /// that tenant's. `tenant` must not be empty (the default tenant holds
    /// the platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// A gateway naming no tenant: for a store that is one tenant's already
    /// (a storefront's connection, which the store API serves in its key's
    /// tenant whatever its requests name). Every call then acts in the
    /// default tenant, which that store maps to its own.
    pub fn unscoped(executor: E) -> Self {
        Self {
            executor,
            tenant: String::new(),
        }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The store the tenant's streams live in.
    pub fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

/// One picture of a page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageImage {
    pub id: String,
    pub ext: String,
    pub mime: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl PageImage {
    /// The name it is kept and written into the body under.
    pub fn file(&self) -> String {
        format!("{}.{}", self.id, self.ext)
    }
}

/// The write model of a page, and what a storefront shows of it.
#[evento::projection(id = id, name = "timada-pages/PageState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct PageState {
    pub id: String,
    pub slug: String,
    pub title: String,
    /// Markdown.
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
    pub published: bool,
    /// Where it is listed in the footer, while it is.
    pub footer_position: Option<u32>,
    pub images: Vec<PageImage>,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
    pub deleted: bool,
}

impl PageState {
    /// The title search engines show: the page's own unless one was set.
    pub fn seo_title(&self) -> &str {
        if self.meta_title.is_empty() {
            &self.title
        } else {
            &self.meta_title
        }
    }

    /// Whether a storefront lists it in its footer.
    pub fn in_footer(&self) -> bool {
        self.published && self.footer_position.is_some()
    }
}

pub fn page_projection<E: Executor>() -> Projection<E, PageState> {
    Projection::new::<crate::aggregator::Page>()
        .handler(on_created())
        .handler(on_edited())
        .handler(on_seo_set())
        .handler(on_renamed())
        .handler(on_published())
        .handler(on_unpublished())
        .handler(on_shown_in_footer())
        .handler(on_hidden_from_footer())
        .handler(on_image_added())
        .handler(on_image_removed())
        .handler(on_deleted())
        .strict()
}

#[evento::handler]
async fn on_created(event: Event<Created>, page: &mut PageState) -> anyhow::Result<()> {
    page.id = event.aggregate_id.to_owned();
    page.slug = event.data.slug.to_owned();
    page.title = event.data.title.to_owned();
    page.body = event.data.body.to_owned();
    page.created_at = event.timestamp;
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_edited(event: Event<Edited>, page: &mut PageState) -> anyhow::Result<()> {
    page.title = event.data.title.to_owned();
    page.body = event.data.body.to_owned();
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_seo_set(event: Event<SeoSet>, page: &mut PageState) -> anyhow::Result<()> {
    page.meta_title = event.data.meta_title.to_owned();
    page.meta_description = event.data.meta_description.to_owned();
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_renamed(event: Event<Renamed>, page: &mut PageState) -> anyhow::Result<()> {
    page.slug = event.data.slug.to_owned();
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_published(event: Event<Published>, page: &mut PageState) -> anyhow::Result<()> {
    page.published = true;
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_unpublished(event: Event<Unpublished>, page: &mut PageState) -> anyhow::Result<()> {
    page.published = false;
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_shown_in_footer(
    event: Event<ShownInFooter>,
    page: &mut PageState,
) -> anyhow::Result<()> {
    page.footer_position = Some(event.data.position);
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_hidden_from_footer(
    event: Event<HiddenFromFooter>,
    page: &mut PageState,
) -> anyhow::Result<()> {
    page.footer_position = None;
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_image_added(event: Event<ImageAdded>, page: &mut PageState) -> anyhow::Result<()> {
    let data = &event.data;
    page.images.push(PageImage {
        id: data.image_id.to_owned(),
        ext: data.ext.to_owned(),
        mime: data.mime.to_owned(),
        size: data.size,
        width: data.width,
        height: data.height,
    });
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_image_removed(event: Event<ImageRemoved>, page: &mut PageState) -> anyhow::Result<()> {
    page.images.retain(|image| image.id != event.data.image_id);
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_deleted(event: Event<Deleted>, page: &mut PageState) -> anyhow::Result<()> {
    page.deleted = true;
    page.published = false;
    page.footer_position = None;
    page.updated_at = event.timestamp;
    Ok(())
}

#[evento::projection(id = id, name = "timada-pages/PageSlugState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct PageSlugState {
    pub id: String,
    pub page_id: String,
    pub released: bool,
}

pub fn page_slug_projection<E: Executor>() -> Projection<E, PageSlugState> {
    Projection::new::<crate::aggregator::PageSlug>()
        .handler(on_slug_claimed())
        .handler(on_slug_released())
        .strict()
}

#[evento::handler]
async fn on_slug_claimed(event: Event<Claimed>, index: &mut PageSlugState) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.page_id = event.data.page_id.to_owned();
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_slug_released(
    _event: Event<Released>,
    index: &mut PageSlugState,
) -> anyhow::Result<()> {
    index.released = true;
    Ok(())
}

impl<E: Executor> Command<E> {
    /// A page that is not deleted.
    pub async fn load_page(&self, page_id: &str) -> anyhow::Result<Option<PageState>> {
        Ok(self
            .load_page_any(page_id)
            .await?
            .filter(|page| !page.deleted))
    }

    pub(crate) async fn load_page_any(&self, page_id: &str) -> anyhow::Result<Option<PageState>> {
        page_projection()
            .load(page_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The page a slug names, published or not.
    pub async fn load_page_by_slug(&self, slug: &str) -> anyhow::Result<Option<PageState>> {
        let Some(index) = self.load_page_slug_index(slug).await? else {
            return Ok(None);
        };
        self.load_page(&index.page_id).await
    }

    /// The live index entry of a slug (released ones are `None`).
    pub async fn load_page_slug_index(&self, slug: &str) -> anyhow::Result<Option<PageSlugState>> {
        Ok(self
            .load_page_slug_index_any(slug)
            .await?
            .filter(|index| !index.released))
    }

    pub(crate) async fn load_page_slug_index_any(
        &self,
        slug: &str,
    ) -> anyhow::Result<Option<PageSlugState>> {
        page_slug_projection()
            .load(page_slug_id(slug))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
