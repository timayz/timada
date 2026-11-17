//! What a product or a category says in one more language of the shop.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use timada_core::claim::claim_is_fresh;
use timada_core::languages::is_language;
use timada_core::{FormerSlug, leave_slug};

use crate::aggregator::translation::{
    BodyWritten, Claimed, Described, Dropped, Named, QuestionsSet, SeoSet, Translation,
    TranslationSlug,
};
use crate::error::CatalogError;
use crate::value_object::{
    CategoryQuestion, FALLBACK_CATEGORY_SLUG, TranslationKind, normalize_category_body,
    normalize_category_description, normalize_category_name, normalize_category_questions,
    normalize_description, normalize_name, normalize_seo, normalize_slug, slugify, slugify_or,
    suffixed_slug, translation_id, translation_slug_id,
};

use super::{CLAIM_ATTEMPTS, Command};

/// Tries at the derived slug, then with fresh random suffixes.
const SLUG_ATTEMPTS: usize = 4;

/// What a product or a category says in one language, as a merchant fills
/// it in. Only the name is needed; what is left empty is read in the main
/// language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranslationText {
    pub name: String,
    /// The URL handle in this language. `None` or blank keeps the one the
    /// translation has, and derives one from the name the first time (with
    /// a suffix when it is taken); a given one that is taken is refused.
    pub slug: Option<String>,
    pub description: String,
    pub meta_title: String,
    pub meta_description: String,
    /// The buying guide below a category's products; a product has none.
    pub body: String,
    /// The questions a category's page answers; a product has none.
    pub questions: Vec<CategoryQuestion>,
}

/// The write model of one translation, and what a storefront copies of it.
#[evento::projection(id = id, name = "timada-catalog/TranslationState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TranslationState {
    pub id: String,
    /// `product` or `category` ([`TranslationKind`]).
    pub kind: String,
    pub subject_id: String,
    pub language: String,
    pub name: String,
    /// Empty once dropped.
    pub slug: String,
    /// The slugs it went by in this language before, oldest first.
    pub former_slugs: Vec<FormerSlug>,
    pub description: String,
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
    pub questions: Vec<CategoryQuestion>,
    /// Gone: the thing is read in the main language. The former slugs
    /// stay, for an old address to be answered from.
    pub dropped: bool,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl TranslationState {
    /// Whether the translation still goes by `slug`.
    pub fn holds_slug(&self, slug: &str) -> bool {
        !self.dropped && self.slug == slug
    }
}

pub fn translation_projection<E: Executor>() -> Projection<E, TranslationState> {
    Projection::new::<Translation>()
        .handler(on_named())
        .handler(on_described())
        .handler(on_body_written())
        .handler(on_seo_set())
        .handler(on_questions_set())
        .handler(on_dropped())
        .strict()
}

#[evento::handler]
async fn on_named(event: Event<Named>, state: &mut TranslationState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.kind = event.data.kind.to_owned();
    state.subject_id = event.data.subject_id.to_owned();
    state.language = event.data.language.to_owned();
    state.name = event.data.name.to_owned();
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
    state.dropped = false;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_described(event: Event<Described>, state: &mut TranslationState) -> anyhow::Result<()> {
    state.description = event.data.description.to_owned();
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_body_written(
    event: Event<BodyWritten>,
    state: &mut TranslationState,
) -> anyhow::Result<()> {
    state.body = event.data.body.to_owned();
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_seo_set(event: Event<SeoSet>, state: &mut TranslationState) -> anyhow::Result<()> {
    state.meta_title = event.data.meta_title.to_owned();
    state.meta_description = event.data.meta_description.to_owned();
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_questions_set(
    event: Event<QuestionsSet>,
    state: &mut TranslationState,
) -> anyhow::Result<()> {
    state.questions = event.data.questions.to_owned();
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_dropped(event: Event<Dropped>, state: &mut TranslationState) -> anyhow::Result<()> {
    let slug = std::mem::take(&mut state.slug);
    leave_slug(&mut state.former_slugs, &slug, "", event.timestamp);
    state.name.clear();
    state.description.clear();
    state.body.clear();
    state.meta_title.clear();
    state.meta_description.clear();
    state.questions.clear();
    state.dropped = true;
    state.updated_at = event.timestamp;
    Ok(())
}

/// The index entry of a slug in one language.
#[evento::projection(id = id, name = "timada-catalog/TranslationSlugState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct TranslationSlugState {
    pub id: String,
    pub subject_id: String,
    /// The version of the translation the last claim was made at.
    pub version: u16,
    /// Unix seconds of the last claim.
    pub claimed_at: u64,
}

pub fn translation_slug_projection<E: Executor>() -> Projection<E, TranslationSlugState> {
    Projection::new::<TranslationSlug>()
        .handler(on_slug_claimed())
        .strict()
}

#[evento::handler]
async fn on_slug_claimed(
    event: Event<Claimed>,
    index: &mut TranslationSlugState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.subject_id = event.data.subject_id.to_owned();
    index.version = event.data.version;
    index.claimed_at = event.timestamp;
    Ok(())
}

/// A language code from the list, lowercased; `InvalidLanguage` for any
/// other.
fn normalize_language(code: &str) -> Result<String, CatalogError> {
    let code = code.trim().to_ascii_lowercase();
    if is_language(&code) {
        Ok(code)
    } else {
        Err(CatalogError::InvalidLanguage)
    }
}

impl<E: Executor> Command<E> {
    /// What the thing says in `language`; `None` when it has no
    /// translation there, or dropped it.
    pub async fn load_translation(
        &self,
        kind: TranslationKind,
        subject_id: &str,
        language: &str,
    ) -> anyhow::Result<Option<TranslationState>> {
        Ok(self
            .load_translation_any(kind, subject_id, language)
            .await?
            .filter(|translation| !translation.dropped))
    }

    /// The translation, dropped or not.
    pub async fn load_translation_any(
        &self,
        kind: TranslationKind,
        subject_id: &str,
        language: &str,
    ) -> anyhow::Result<Option<TranslationState>> {
        translation_projection()
            .load(translation_id(kind, subject_id, language))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// What the thing says in each of `languages` it is translated into,
    /// in their order.
    pub async fn load_translations(
        &self,
        kind: TranslationKind,
        subject_id: &str,
        languages: &[String],
    ) -> anyhow::Result<Vec<TranslationState>> {
        let mut translations = Vec::new();
        for language in languages {
            if let Some(translation) = self.load_translation(kind, subject_id, language).await? {
                translations.push(translation);
            }
        }
        Ok(translations)
    }

    async fn load_translation_slug(
        &self,
        kind: TranslationKind,
        language: &str,
        slug: &str,
    ) -> anyhow::Result<Option<TranslationSlugState>> {
        translation_slug_projection()
            .load(translation_slug_id(kind, language, slug))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// Whether the thing is still there to be read: a product neither
    /// archived nor deleted, a category not archived.
    async fn subject_is_live(
        &self,
        kind: TranslationKind,
        subject_id: &str,
    ) -> anyhow::Result<bool> {
        Ok(match kind {
            TranslationKind::Product => self
                .load_product(subject_id)
                .await?
                .is_some_and(|product| !product.is_archived()),
            TranslationKind::Category => self
                .load_category(subject_id)
                .await?
                .is_some_and(|category| !category.archived),
        })
    }

    /// Whether another thing of the kind goes by `slug` in the main
    /// language. Its address in a language it has no translation in is that
    /// slug under the language's prefix, which a translation must not take.
    async fn main_slug_of_another(
        &self,
        kind: TranslationKind,
        slug: &str,
        subject_id: &str,
    ) -> anyhow::Result<bool> {
        Ok(match kind {
            TranslationKind::Product => self
                .load_product_by_slug(slug)
                .await?
                .is_some_and(|product| product.id != subject_id),
            TranslationKind::Category => self
                .load_category_by_slug(slug)
                .await?
                .is_some_and(|category| category.id != subject_id),
        })
    }

    /// Whether the thing an entry names has let go of the slug in
    /// `language`: gone itself, its translation dropped or moved to another
    /// slug past the claim, or never written once the claim is no longer
    /// fresh.
    async fn translation_slug_let_go(
        &self,
        kind: TranslationKind,
        language: &str,
        slug: &str,
        index: &TranslationSlugState,
    ) -> anyhow::Result<bool> {
        if !self.subject_is_live(kind, &index.subject_id).await? {
            return Ok(true);
        }
        let stale = !claim_is_fresh(index.claimed_at, timada_core::unix_now());
        let Some(translation) = self
            .load_translation_any(kind, &index.subject_id, language)
            .await?
        else {
            return Ok(stale);
        };
        if translation.holds_slug(slug) {
            return Ok(false);
        }
        Ok(translation.aggregate_version()? > index.version || stale)
    }

    /// Points a normalized slug of `language` at the thing, at the version
    /// of its translation the next write is made at. The entry is created,
    /// or taken over when it is already the thing's or held by one that
    /// let go of it. Fails when another thing holds it, in this language or
    /// as its main slug, or wins the race for it.
    async fn claim_translation_slug(
        &self,
        kind: TranslationKind,
        language: &str,
        slug: &str,
        subject_id: &str,
        version: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if self.main_slug_of_another(kind, slug, subject_id).await? {
            return Err(CatalogError::SlugTaken);
        }
        let claimed = Claimed {
            subject_id: subject_id.to_owned(),
            version,
        };
        let created = evento::append(translation_slug_id(kind, language, slug))
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
            let Some(existing) = self.load_translation_slug(kind, language, slug).await? else {
                return Err(CatalogError::SlugTaken);
            };
            let free = existing.subject_id == subject_id
                || self
                    .translation_slug_let_go(kind, language, slug, &existing)
                    .await?;
            if !free {
                return Err(CatalogError::SlugTaken);
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
        Err(CatalogError::SlugTaken)
    }

    /// Claims the slug a name gives, with a random suffix when it is taken,
    /// and returns the one it got.
    async fn claim_derived_translation_slug(
        &self,
        kind: TranslationKind,
        language: &str,
        base: String,
        subject_id: &str,
        version: u16,
        routing_key: Option<String>,
    ) -> Result<String, CatalogError> {
        let mut slug = base.clone();
        for attempt in 0..SLUG_ATTEMPTS {
            match self
                .claim_translation_slug(
                    kind,
                    language,
                    &slug,
                    subject_id,
                    version,
                    routing_key.clone(),
                )
                .await
            {
                Ok(()) => break,
                Err(CatalogError::SlugTaken) if attempt + 1 < SLUG_ATTEMPTS => {
                    slug = suffixed_slug(&base);
                }
                Err(err) => return Err(err),
            }
        }
        Ok(slug)
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Words a product or a category in `language`, whole: what `text`
    /// leaves empty is read in the main language, and only what changed is
    /// written. The thing has to be live. A language is one of the list,
    /// whether the shop's storefront is read in it yet or not: a merchant
    /// may word a language before opening it. A slug claimed for a write
    /// that then fails is not given back: the next claim takes it over
    /// once the translation is written past it, or the claim is no longer
    /// fresh.
    pub async fn translate(
        &self,
        actor: &str,
        kind: TranslationKind,
        subject_id: &str,
        language: &str,
        text: TranslationText,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let language = normalize_language(language)?;
        let (name, description, body, questions, fallback_slug) = match kind {
            TranslationKind::Product => {
                self.require_editable(subject_id).await?;
                (
                    normalize_name(&text.name)?,
                    normalize_description(&text.description)?,
                    String::new(),
                    Vec::new(),
                    None,
                )
            }
            TranslationKind::Category => {
                self.require_live_category(subject_id).await?;
                (
                    normalize_category_name(&text.name)?,
                    normalize_category_description(&text.description)?,
                    normalize_category_body(&text.body)?,
                    normalize_category_questions(&text.questions)?,
                    Some(FALLBACK_CATEGORY_SLUG),
                )
            }
        };
        let (meta_title, meta_description) =
            normalize_seo(&text.meta_title, &text.meta_description)?;
        let given = text
            .slug
            .as_deref()
            .map(str::trim)
            .filter(|slug| !slug.is_empty())
            .map(normalize_slug)
            .transpose()?;

        let state = self
            .load_translation_any(kind, subject_id, &language)
            .await?;
        let version = match &state {
            Some(state) => state.aggregate_version()?,
            None => 0,
        };
        let current = state
            .as_ref()
            .filter(|state| !state.dropped)
            .map(|state| state.slug.clone());
        let slug = match (given, current) {
            (Some(given), Some(current)) if given == current => current,
            (Some(given), _) => {
                self.claim_translation_slug(
                    kind,
                    &language,
                    &given,
                    subject_id,
                    version,
                    routing_key.clone(),
                )
                .await?;
                given
            }
            (None, Some(current)) => current,
            (None, None) => {
                let base = match fallback_slug {
                    Some(fallback) => slugify_or(&name, fallback),
                    None => slugify(&name),
                };
                self.claim_derived_translation_slug(
                    kind,
                    &language,
                    base,
                    subject_id,
                    version,
                    routing_key.clone(),
                )
                .await?
            }
        };

        let live = state.as_ref().filter(|state| !state.dropped);
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(translation_id(kind, subject_id, &language));
                write.original_version(0);
                write
            }
        };
        let mut changed = false;
        if live.is_none_or(|was| was.name != name || was.slug != slug) {
            write.event(&Named {
                kind: kind.as_str().to_owned(),
                subject_id: subject_id.to_owned(),
                language: language.clone(),
                name,
                slug,
                named_by: actor.to_owned(),
            });
            changed = true;
        }
        // A dropped translation was emptied: only what is said is written.
        if live.map_or(!description.is_empty(), |was| {
            was.description != description
        }) {
            write.event(&Described {
                description,
                described_by: actor.to_owned(),
            });
            changed = true;
        }
        if live.map_or(!body.is_empty(), |was| was.body != body) {
            write.event(&BodyWritten {
                body,
                written_by: actor.to_owned(),
            });
            changed = true;
        }
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
        if live.map_or(!questions.is_empty(), |was| was.questions != questions) {
            write.event(&QuestionsSet {
                questions,
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

    /// Drops what the thing says in `language`: it is read in the main
    /// language there again, and the slug it had is free. One that has
    /// none is a no-op; the thing itself may be archived by then.
    pub async fn drop_translation(
        &self,
        actor: &str,
        kind: TranslationKind,
        subject_id: &str,
        language: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let language = normalize_language(language)?;
        let Some(state) = self.load_translation(kind, subject_id, &language).await? else {
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
