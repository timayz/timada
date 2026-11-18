//! What the shop says in its other languages for the words its products
//! share: the names of their options, the values these take, the labels
//! and the values of the facts of their tables.

use std::collections::HashMap;

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::translation::{Unworded, Word, Worded};
use crate::error::CatalogError;
use crate::value_object::{normalize_word, word_id, word_key};

use super::translation::normalize_language;
use super::{Command, ProductState, ProductVariant};

/// The write model of one word in one language, and what a storefront
/// copies of it.
#[evento::projection(id = id, name = "timada-catalog/WordState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct WordState {
    pub id: String,
    pub language: String,
    /// The word as the main language writes it, as it was last worded.
    pub term: String,
    /// What `language` reads for it; empty once it is read as the main
    /// language writes it again.
    pub wording: String,
}

impl WordState {
    /// What two words are compared by ([`word_key`]).
    pub fn key(&self) -> String {
        word_key(&self.term)
    }
}

pub fn word_projection<E: Executor>() -> Projection<E, WordState> {
    Projection::new::<Word>()
        .handler(on_worded())
        .handler(on_unworded())
        .strict()
}

#[evento::handler]
async fn on_worded(event: Event<Worded>, state: &mut WordState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.language = event.data.language.to_owned();
    state.term = event.data.term.to_owned();
    state.wording = event.data.wording.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_unworded(_event: Event<Unworded>, state: &mut WordState) -> anyhow::Result<()> {
    state.wording.clear();
    Ok(())
}

impl ProductState {
    /// The words a shopper reads on the product beside its name and its
    /// description, each once, in the order a page shows them: its options
    /// and the values they take, as their labels show them, then the facts
    /// of its table. What the shop words in its other languages
    /// ([`Command::word`]).
    pub fn words(&self) -> Vec<String> {
        let mut words: Vec<String> = Vec::new();
        let mut note = |word: &str| {
            let key = word_key(word);
            if !key.is_empty() && !words.iter().any(|seen| word_key(seen) == key) {
                words.push(word.trim().to_owned());
            }
        };
        for option in self.shown_options() {
            note(option.display_name());
            for value in &option.values {
                note(option.display_value(value));
            }
        }
        for attribute in &self.attributes {
            note(&attribute.name);
            note(&attribute.value);
        }
        words
    }

    /// [`variant_label`](Self::variant_label) as `words` reads it: each
    /// value by the wording its key has there, else as the option shows it.
    pub fn variant_label_in(
        &self,
        variant: &ProductVariant,
        words: &HashMap<String, String>,
    ) -> Option<String> {
        let values: Vec<&str> = self
            .shown_selections(variant)
            .map(|selection| {
                let shown = self
                    .option(&selection.option_id)
                    .map_or(selection.value.as_str(), |option| {
                        option.display_value(&selection.value)
                    });
                words.get(&word_key(shown)).map_or(shown, String::as_str)
            })
            .collect();
        (!values.is_empty()).then(|| values.join(" / "))
    }
}

impl<E: Executor> Command<E> {
    async fn load_word_state(
        &self,
        language: &str,
        term: &str,
    ) -> anyhow::Result<Option<WordState>> {
        word_projection()
            .load(word_id(language, term))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// What `language` reads for each of `terms` the shop worded there,
    /// by the word's key ([`word_key`]): a word with no entry is read as
    /// the main language writes it. One read a word, each by its id.
    pub async fn load_words(
        &self,
        language: &str,
        terms: &[String],
    ) -> anyhow::Result<HashMap<String, String>> {
        let mut words = HashMap::new();
        for term in terms {
            let key = word_key(term);
            if key.is_empty() || words.contains_key(&key) {
                continue;
            }
            if let Some(word) = self.load_word_state(language, term).await?
                && !word.wording.is_empty()
            {
                words.insert(key, word.wording);
            }
        }
        Ok(words)
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Says what `language` reads for `term`, a word of the shop's
    /// products, wherever it shows: on every product that carries it and in
    /// the filters of the listings. An empty `wording` takes the entry
    /// back; the same one is a no-op. A language is one of the list,
    /// whether the storefront is read in it yet or not; the word need not
    /// be on a product yet either.
    pub async fn word(
        &self,
        actor: &str,
        language: &str,
        term: &str,
        wording: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let language = normalize_language(language)?;
        let term = normalize_word(term)?;
        let wording = normalize_word(wording)?;
        if term.is_empty() {
            return Err(CatalogError::InvalidWord);
        }
        let state = self.load_word_state(&language, &term).await?;
        let mut write = match &state {
            Some(state) if state.wording == wording => return Ok(()),
            Some(state) => state.write()?,
            None if wording.is_empty() => return Ok(()),
            None => {
                let mut write = evento::append(word_id(&language, &term));
                write.original_version(0);
                write
            }
        };
        if wording.is_empty() {
            write.event(&Unworded {
                unworded_by: actor.to_owned(),
            });
        } else {
            write.event(&Worded {
                language,
                term,
                wording,
                worded_by: actor.to_owned(),
            });
        }
        write
            .routing_key_opt(routing_key)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
