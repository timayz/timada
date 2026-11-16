use std::collections::BTreeSet;

use evento::ProjectionAggregate;
use evento::{Executor, WriteError};
use timada_core::claim::claim_is_fresh;

use crate::aggregator::article::{Article, ArticleEvent, Claimed, Released};
use crate::error::PageError;
use crate::value_object::article_slug_id;

use super::slug_index::CLAIM_ATTEMPTS;
use super::{ArticleSlugState, Command};

impl<E: Executor> Command<E> {
    /// Points a normalized slug at `article_id`, at the version of the article its
    /// next write is made at. The entry is created, or taken over when it is
    /// free, already the article's (the claim is renewed, so a release of what
    /// the article gave up earlier leaves it alone), or held by an article that let
    /// go of it. Fails when another article holds it, or wins the race for it.
    pub(crate) async fn claim_article_slug(
        &self,
        slug: &str,
        article_id: &str,
        article_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let claimed = Claimed {
            article_id: article_id.to_owned(),
            article_version,
        };
        let created = evento::append(article_slug_id(slug))
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
            let Some(existing) = self.load_article_slug_index_any(slug).await? else {
                return Err(PageError::ArticleSlugTaken);
            };
            let free = existing.released
                || existing.article_id == article_id
                || self.article_slug_let_go(slug, &existing).await?;
            if !free {
                return Err(PageError::ArticleSlugTaken);
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
        Err(PageError::ArticleSlugTaken)
    }

    /// Whether the article an entry names has let go of the slug: deleted,
    /// written past its claim without taking the slug, or never written at
    /// all once the claim is no longer fresh.
    async fn article_slug_let_go(
        &self,
        slug: &str,
        index: &ArticleSlugState,
    ) -> anyhow::Result<bool> {
        let Some(article) = self.load_article_any(&index.article_id).await? else {
            return Ok(!claim_is_fresh(index.claimed_at, timada_core::unix_now()));
        };
        if article.deleted {
            return Ok(true);
        }
        if article.holds_slug(slug) {
            return Ok(false);
        }
        Ok(article.aggregate_version()? > index.article_version
            || !claim_is_fresh(index.claimed_at, timada_core::unix_now()))
    }

    /// Gives back a claim the article could not use: its write after the claim
    /// at `article_version` failed, and the article has been written past that
    /// version without the slug. While it is still at that version the
    /// write may land yet (the store could not tell), so the claim stays:
    /// the next claim takes it over once the article writes past it, or once
    /// it is no longer fresh.
    pub(crate) async fn release_article_slug_claim(
        &self,
        slug: &str,
        article_id: &str,
        article_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let Some(article) = self.load_article_any(article_id).await? else {
            return Ok(());
        };
        if article.holds_slug(slug) || article.aggregate_version()? <= article_version {
            return Ok(());
        }
        self.release_article_slug(
            slug,
            article_id,
            article_version.saturating_add(1),
            routing_key,
        )
        .await
    }

    /// Frees the entry of a slug the article claimed before its version
    /// `before` and no longer holds (the caller checks that). An entry
    /// already free, another article's, or claimed again since is left alone,
    /// so an article taking a slug back keeps it.
    async fn release_article_slug(
        &self,
        slug: &str,
        article_id: &str,
        before: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(index) = self.load_article_slug_index(slug).await? else {
                return Ok(());
            };
            if index.article_id != article_id || index.article_version >= before {
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

    /// Frees every slug the article claimed before its version `before` and
    /// holds no more: what renaming or deleting it gave up, for the release
    /// subscription ([`crate::release_subscription`]). The slugs come from
    /// its whole history, so one release missed earlier is made up for too.
    /// Told twice, nothing more happens; a slug the article took back since is
    /// kept.
    pub(crate) async fn release_article_given_up(
        &self,
        article_id: &str,
        before: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let Some(article) = self.load_article_any(article_id).await? else {
            return Ok(());
        };
        let mut slugs = BTreeSet::new();
        let history = evento::read::<Article>(article_id)
            .tenant(&self.tenant)
            .decode(&self.executor)
            .await?;
        for event in history {
            match event {
                ArticleEvent::Created(created) => {
                    slugs.insert(created.slug);
                }
                ArticleEvent::Renamed(renamed) => {
                    slugs.insert(renamed.slug);
                }
                _ => {}
            }
        }
        for slug in slugs.iter().filter(|slug| !article.holds_slug(slug)) {
            self.release_article_slug(slug, article_id, before, routing_key.clone())
                .await?;
        }
        Ok(())
    }
}
