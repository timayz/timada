//! The languages a shop's storefront is read in.

use evento::{Executor, ProjectionAggregate, metadata::Event};
use timada_core::languages::{is_language, language_rank};

use crate::aggregator::LanguagesSet;
use crate::error::PageError;
use crate::value_object::{MAX_OTHER_LANGUAGES, languages_id};

/// The languages a storefront is read in, as ISO 639-1 codes from
/// [`LANGUAGES`](timada_core::languages::LANGUAGES).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopLanguages {
    /// The one the products, the categories, the pages and the articles
    /// are written in on their own streams.
    pub main: String,
    /// The ones they are translated into, in the list's order; never
    /// `main`.
    pub others: Vec<String>,
}

impl ShopLanguages {
    /// Whether the storefront is read in `language`.
    pub fn has(&self, language: &str) -> bool {
        self.main == language || self.others.iter().any(|other| other == language)
    }
}

/// The write model of the shop's languages, and what a storefront copies of
/// it.
#[evento::projection(id = id, name = "timada-pages/LanguagesState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct LanguagesState {
    pub id: String,
    pub main: String,
    pub others: Vec<String>,
}

pub fn languages_projection<E: Executor>() -> evento::Projection<E, LanguagesState> {
    evento::Projection::new::<crate::aggregator::Languages>()
        .handler(on_languages_set())
        .strict()
}

#[evento::handler]
async fn on_languages_set(
    event: Event<LanguagesSet>,
    state: &mut LanguagesState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.main = event.data.main.clone();
    state.others = event.data.others.clone();
    Ok(())
}

/// A language code from the list, lowercased; `InvalidLanguage` for any
/// other.
pub fn normalize_language(code: &str) -> Result<String, PageError> {
    let code = code.trim().to_ascii_lowercase();
    if is_language(&code) {
        Ok(code)
    } else {
        Err(PageError::InvalidLanguage)
    }
}

impl<E: Executor> super::Command<E> {
    async fn load_languages_state(&self) -> anyhow::Result<Option<LanguagesState>> {
        languages_projection()
            .load(languages_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The languages the storefront is read in; `None` while the shop never
    /// set them, which leaves it in its storefront's own language.
    pub async fn languages(&self) -> anyhow::Result<Option<ShopLanguages>> {
        Ok(self
            .load_languages_state()
            .await?
            .map(|state| ShopLanguages {
                main: state.main,
                others: state.others,
            }))
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets the languages the storefront is read in: the main one, and the
    /// others in the list's order, the main one and repeats among them
    /// dropped, [`MAX_OTHER_LANGUAGES`] at most. A language taken off keeps
    /// its translations, out of use, for the day it comes back. The same
    /// languages are a no-op.
    pub async fn set_languages(
        &self,
        actor: &str,
        main: &str,
        others: &[String],
        routing_key: Option<String>,
    ) -> Result<(), PageError> {
        let main = normalize_language(main)?;
        let mut wanted: Vec<String> = Vec::new();
        for other in others {
            let other = normalize_language(other)?;
            if other != main && !wanted.contains(&other) {
                wanted.push(other);
            }
        }
        if wanted.len() > MAX_OTHER_LANGUAGES {
            return Err(PageError::TooManyLanguages);
        }
        wanted.sort_by_key(|code| language_rank(code));
        let state = self.load_languages_state().await?;
        let mut write = match &state {
            Some(state) if state.main == main && state.others == wanted => return Ok(()),
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(languages_id());
                write.original_version(0);
                write
            }
        };
        write
            .routing_key_opt(routing_key)
            .event(&LanguagesSet {
                main,
                others: wanted,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
