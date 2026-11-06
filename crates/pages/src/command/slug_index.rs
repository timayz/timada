use std::collections::BTreeSet;

use evento::ProjectionAggregate;
use evento::{Executor, WriteError};
use timada_core::claim::claim_is_fresh;

use crate::aggregator::{Claimed, Page, PageEvent, Released};
use crate::error::PageError;
use crate::value_object::page_slug_id;

use super::{Command, PageSlugState};

/// How many times a claim or a release reads the entry again after losing
/// a race for it.
const CLAIM_ATTEMPTS: usize = 3;

impl<E: Executor> Command<E> {
    /// Points a normalized slug at `page_id`, at the version of the page its
    /// next write is made at. The entry is created, or taken over when it is
    /// free, already the page's (the claim is renewed, so a release of what
    /// the page gave up earlier leaves it alone), or held by a page that let
    /// go of it. Fails when another page holds it, or wins the race for it.
    pub(crate) async fn claim_page_slug(
        &self,
        slug: &str,
        page_id: &str,
        page_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let claimed = Claimed {
            page_id: page_id.to_owned(),
            page_version,
        };
        let created = evento::append(page_slug_id(slug))
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&claimed)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match created {
            Ok(_) => return Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(existing) = self.load_page_slug_index_any(slug).await? else {
                return Err(PageError::SlugTaken);
            };
            let free = existing.released
                || existing.page_id == page_id
                || self.page_slug_let_go(slug, &existing).await?;
            if !free {
                return Err(PageError::SlugTaken);
            }
            let taken = existing
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&claimed)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match taken {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(PageError::SlugTaken)
    }

    /// Whether the page an entry names has let go of the slug: deleted,
    /// written past its claim without taking the slug, or never written at
    /// all once the claim is no longer fresh.
    async fn page_slug_let_go(&self, slug: &str, index: &PageSlugState) -> anyhow::Result<bool> {
        let Some(page) = self.load_page_any(&index.page_id).await? else {
            return Ok(!claim_is_fresh(index.claimed_at, timada_core::unix_now()));
        };
        if page.deleted {
            return Ok(true);
        }
        if page.holds_slug(slug) {
            return Ok(false);
        }
        Ok(page.aggregate_version()? > index.page_version
            || !claim_is_fresh(index.claimed_at, timada_core::unix_now()))
    }

    /// Gives back a claim the page could not use: its write after the claim
    /// at `page_version` failed. Left alone when the page holds the slug
    /// after all.
    pub(crate) async fn release_page_slug_claim(
        &self,
        slug: &str,
        page_id: &str,
        page_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let held = self
            .load_page_any(page_id)
            .await?
            .is_some_and(|page| page.holds_slug(slug));
        if held {
            return Ok(());
        }
        self.release_page_slug(slug, page_id, page_version.saturating_add(1), routing_key)
            .await
    }

    /// Frees the entry of a slug the page claimed before its version
    /// `before` and no longer holds (the caller checks that). An entry
    /// already free, another page's, or claimed again since is left alone,
    /// so a page taking a slug back keeps it.
    async fn release_page_slug(
        &self,
        slug: &str,
        page_id: &str,
        before: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(index) = self.load_page_slug_index(slug).await? else {
                return Ok(());
            };
            if index.page_id != page_id || index.page_version >= before {
                return Ok(());
            }
            let released = index
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Released)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match released {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// Frees every slug the page claimed before its version `before` and
    /// holds no more: what renaming or deleting it gave up, for the release
    /// subscription ([`crate::release_subscription`]). The slugs come from
    /// its whole history, so one release missed earlier is made up for too.
    /// Told twice, nothing more happens; a slug the page took back since is
    /// kept.
    pub(crate) async fn release_given_up(
        &self,
        page_id: &str,
        before: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let Some(page) = self.load_page_any(page_id).await? else {
            return Ok(());
        };
        let mut slugs = BTreeSet::new();
        let history = evento::read::<Page>(page_id)
            .tenant(&self.tenant)
            .decode(&self.executor)
            .await?;
        for event in history {
            match event {
                PageEvent::Created(created) => {
                    slugs.insert(created.slug);
                }
                PageEvent::Renamed(renamed) => {
                    slugs.insert(renamed.slug);
                }
                _ => {}
            }
        }
        for slug in slugs.iter().filter(|slug| !page.holds_slug(slug)) {
            self.release_page_slug(slug, page_id, before, routing_key.clone())
                .await?;
        }
        Ok(())
    }
}
