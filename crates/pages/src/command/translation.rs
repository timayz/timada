//! What a page or an article says in one more language of the shop.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use timada_core::claim::claim_is_fresh;
use timada_core::{FormerSlug, leave_slug};

use crate::aggregator::translation::{
    Claimed, Dropped, SeoSet, Translation, TranslationSlug, Written,
};
use crate::error::PageError;
use crate::value_object::{
    TextKind, article_slug, normalize_body, normalize_seo, normalize_slug, normalize_summary,
    normalize_title, text_translation_id, text_translation_slug_id,
};

use super::slug_index::CLAIM_ATTEMPTS;
use super::{Command, normalize_language};

/// What a page or an article says in one language, as a merchant fills it
/// in. Only the title is needed; what is left empty is read in the main
/// language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextTranslation {
    pub title: String,
    /// What an article's address goes by in this language. `None` or blank
    /// keeps the one the translation has, and takes the one its title
    /// gives the first time; one that is taken is refused. A page has
    /// none.
    pub slug: Option<String>,
    /// An article's, for the blog's list; a page has none.
    pub summary: String,
    /// Markdown.
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
}

/// The write model of one translation, and what a storefront copies of it.
#[evento::projection(id = id, name = "timada-pages/TranslationState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TextTranslationState {
    pub id: String,
    /// `page` or `article` ([`TextKind`]).
    pub kind: String,
    pub subject_id: String,
    pub language: String,
    pub title: String,
    /// An article's slug in this language; empty for a page, and once
    /// dropped.
    pub slug: String,
    /// The slugs it went by in this language before, oldest first.
    pub former_slugs: Vec<FormerSlug>,
    pub summary: String,
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
    /// Gone: the text is read in the main language. The former slugs stay,
    /// for an old address to be answered from.
    pub dropped: bool,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl TextTranslationState {
    /// Whether the translation still goes by `slug`.
    pub fn holds_slug(&self, slug: &str) -> bool {
        !self.dropped && self.slug == slug
    }
}

pub fn text_translation_projection<E: Executor>() -> Projection<E, TextTranslationState> {
    Projection::new::<Translation>()
        .handler(on_written())
        .handler(on_seo_set())
        .handler(on_dropped())
        .strict()
}

#[evento::handler]
async fn on_written(event: Event<Written>, state: &mut TextTranslationState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.kind = event.data.kind.to_owned();
    state.subject_id = event.data.subject_id.to_owned();
    state.language = event.data.language.to_owned();
    state.title = event.data.title.to_owned();
    if state.slug.is_empty() {
        // The first slug, or the one after a drop: nothing is left for it,
        // and it is no former slug any more.
        state.former_slugs.retain(|was| was.slug != event.data.slug);
    } else {
        leave_slug(
            &mut state.former_slugs,
            &state.slug,
            &event.data.slug,
            event.timestamp,
        );
    }
    state.slug = event.data.slug.to_owned();
    state.summary = event.data.summary.to_owned();
    state.body = event.data.body.to_owned();
    state.dropped = false;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_seo_set(event: Event<SeoSet>, state: &mut TextTranslationState) -> anyhow::Result<()> {
    state.meta_title = event.data.meta_title.to_owned();
    state.meta_description = event.data.meta_description.to_owned();
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_dropped(event: Event<Dropped>, state: &mut TextTranslationState) -> anyhow::Result<()> {
    let slug = std::mem::take(&mut state.slug);
    if !slug.is_empty() {
        leave_slug(&mut state.former_slugs, &slug, "", event.timestamp);
    }
    state.title.clear();
    state.summary.clear();
    state.body.clear();
    state.meta_title.clear();
    state.meta_description.clear();
    state.dropped = true;
    state.updated_at = event.timestamp;
    Ok(())
}

/// The index entry of an article's slug in one language.
#[evento::projection(id = id, name = "timada-pages/TranslationSlugState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TextTranslationSlugState {
    pub id: String,
    pub subject_id: String,
    /// The version of the translation the last claim was made at.
    pub version: u16,
    /// Unix seconds of the last claim.
    pub claimed_at: u64,
}

pub fn text_translation_slug_projection<E: Executor>() -> Projection<E, TextTranslationSlugState> {
    Projection::new::<TranslationSlug>()
        .handler(on_slug_claimed())
        .strict()
}

#[evento::handler]
async fn on_slug_claimed(
    event: Event<Claimed>,
    index: &mut TextTranslationSlugState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.subject_id = event.data.subject_id.to_owned();
    index.version = event.data.version;
    index.claimed_at = event.timestamp;
    Ok(())
}

impl<E: Executor> Command<E> {
    /// What the text says in `language`; `None` when it has no translation
    /// there, or dropped it.
    pub async fn load_text_translation(
        &self,
        kind: TextKind,
        subject_id: &str,
        language: &str,
    ) -> anyhow::Result<Option<TextTranslationState>> {
        Ok(self
            .load_text_translation_any(kind, subject_id, language)
            .await?
            .filter(|translation| !translation.dropped))
    }

    /// The translation, dropped or not.
    pub async fn load_text_translation_any(
        &self,
        kind: TextKind,
        subject_id: &str,
        language: &str,
    ) -> anyhow::Result<Option<TextTranslationState>> {
        text_translation_projection()
            .load(text_translation_id(kind, subject_id, language))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// What the text says in each of `languages` it is translated into, in
    /// their order.
    pub async fn load_text_translations(
        &self,
        kind: TextKind,
        subject_id: &str,
        languages: &[String],
    ) -> anyhow::Result<Vec<TextTranslationState>> {
        let mut translations = Vec::new();
        for language in languages {
            if let Some(translation) = self
                .load_text_translation(kind, subject_id, language)
                .await?
            {
                translations.push(translation);
            }
        }
        Ok(translations)
    }

    /// Whether the article an entry names has let go of the slug in
    /// `language`: deleted, its translation dropped or moved to another
    /// slug past the claim, or never written once the claim is no longer
    /// fresh.
    async fn article_translation_slug_let_go(
        &self,
        language: &str,
        slug: &str,
        index: &TextTranslationSlugState,
    ) -> anyhow::Result<bool> {
        if self.load_article(&index.subject_id).await?.is_none() {
            return Ok(true);
        }
        let stale = !claim_is_fresh(index.claimed_at, timada_core::unix_now());
        let Some(translation) = self
            .load_text_translation_any(TextKind::Article, &index.subject_id, language)
            .await?
        else {
            return Ok(stale);
        };
        if translation.holds_slug(slug) {
            return Ok(false);
        }
        Ok(translation.aggregate_version()? > index.version || stale)
    }

    /// Points a normalized slug of `language` at the article, at the
    /// version of its translation the next write is made at. The entry is
    /// created, or taken over when it is already the article's or held by
    /// one that let go of it. Fails when another article holds it, in this
    /// language or as its main slug (its address in every language it is
    /// not translated into), or wins the race for it.
    async fn claim_article_translation_slug(
        &self,
        language: &str,
        slug: &str,
        article_id: &str,
        version: u16,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        if self
            .load_article_by_slug(slug)
            .await?
            .is_some_and(|article| article.id != article_id)
        {
            return Err(PageError::ArticleSlugTaken);
        }
        let claimed = Claimed {
            subject_id: article_id.to_owned(),
            version,
        };
        let created = evento::append(text_translation_slug_id(language, slug))
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
            let Some(existing) = text_translation_slug_projection()
                .load(text_translation_slug_id(language, slug))
                .tenant(&self.tenant)
                .execute(&self.executor)
                .await?
            else {
                return Err(PageError::ArticleSlugTaken);
            };
            let free = existing.subject_id == article_id
                || self
                    .article_translation_slug_let_go(language, slug, &existing)
                    .await?;
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
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Words a page or an article in `language`, whole: what `text` leaves
    /// empty is read in the main language, and only what changed is
    /// written. The text must not be deleted. A language is one of the
    /// list, whether the shop's storefront is read in it yet or not. An
    /// article's slug claimed for a write that then fails is not given
    /// back: the next claim takes it over once the translation is written
    /// past it, or the claim is no longer fresh.
    pub async fn translate_text(
        &self,
        actor: &str,
        kind: TextKind,
        subject_id: &str,
        language: &str,
        text: TextTranslation,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let language = normalize_language(language)?;
        let title = normalize_title(&text.title)?;
        let body = normalize_body(&text.body)?;
        let (meta_title, meta_description) =
            normalize_seo(&text.meta_title, &text.meta_description)?;
        let summary = match kind {
            TextKind::Page => {
                self.load_page(subject_id)
                    .await?
                    .ok_or(PageError::NotFound)?;
                String::new()
            }
            TextKind::Article => {
                self.load_article(subject_id)
                    .await?
                    .ok_or(PageError::ArticleNotFound)?;
                normalize_summary(&text.summary)?
            }
        };
        let state = self
            .load_text_translation_any(kind, subject_id, &language)
            .await?;
        let version = match &state {
            Some(state) => state.aggregate_version()?,
            None => 0,
        };
        let live = state.as_ref().filter(|state| !state.dropped);
        let slug = match kind {
            TextKind::Page => String::new(),
            TextKind::Article => {
                let given = text
                    .slug
                    .as_deref()
                    .map(str::trim)
                    .filter(|slug| !slug.is_empty())
                    .map(normalize_slug)
                    .transpose()?;
                let current = live.map(|state| state.slug.clone());
                match (given, current) {
                    (Some(given), Some(current)) if given == current => current,
                    (None, Some(current)) => current,
                    (given, _) => {
                        let slug = given.unwrap_or_else(|| article_slug(&title));
                        self.claim_article_translation_slug(
                            &language,
                            &slug,
                            subject_id,
                            version,
                            routing_key.clone(),
                        )
                        .await?;
                        slug
                    }
                }
            }
        };

        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(text_translation_id(kind, subject_id, &language));
                write.original_version(0);
                write
            }
        };
        let mut changed = false;
        if live.is_none_or(|was| {
            was.title != title || was.slug != slug || was.summary != summary || was.body != body
        }) {
            write.event(&Written {
                kind: kind.as_str().to_owned(),
                subject_id: subject_id.to_owned(),
                language: language.clone(),
                title,
                slug,
                summary,
                body,
                written_by: actor.to_owned(),
            });
            changed = true;
        }
        // A dropped translation was emptied: only what is said is written.
        if live.map_or(
            !meta_title.is_empty() || !meta_description.is_empty(),
            |was| was.meta_title != meta_title || was.meta_description != meta_description,
        ) {
            write.event(&SeoSet {
                meta_title,
                meta_description,
                set_by: actor.to_owned(),
            });
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        write
            .routing_key_opt(routing_key)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Drops what the text says in `language`: it is read in the main
    /// language there again, and an article's slug in it is free. One that
    /// has none is a no-op; the text itself may be deleted by then.
    pub async fn drop_text_translation(
        &self,
        actor: &str,
        kind: TextKind,
        subject_id: &str,
        language: &str,
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let language = normalize_language(language)?;
        let Some(state) = self
            .load_text_translation(kind, subject_id, &language)
            .await?
        else {
            return Ok(());
        };
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Dropped {
                dropped_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
