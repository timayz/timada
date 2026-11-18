//! What the shop says in its other languages for the words its products
//! share (`timada-catalog`'s `Word`): the names of their options, the
//! values these take, the labels and the values of the facts of their
//! tables. One row per language and word in `storefront_words`, which the
//! catalog subscription keeps; a word with no row is read as the main
//! language writes it. The filters of a listing are made of the same
//! words, so they read the same rows.

use std::collections::HashMap;

use evento::{Executor, metadata::RawEvent, subscription::Context};
use timada_catalog::aggregator::translation::Word;
use timada_catalog::{word_key, word_projection};
use timada_core::db::{Db, placeholders};
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::store;

/// How many words one statement looks up.
const WORDS_CHUNK: usize = 200;

/// The key of a word's row: the language, then what the word is compared
/// by. One column, so a list of them is read a seek each (the planner
/// walks a two-column key whole for a list on its second column).
fn row_id(language: &str, key: &str) -> String {
    format!("{language}\u{1f}{key}")
}

/// The words `language` reads for `terms`, by the word's key
/// ([`word_key`]): only the ones the shop worded there.
///
/// The words are folded before anything is awaited, so the future holds
/// none of the borrows `terms` came with.
pub fn published_words<'a, 't>(
    db: &'a Db,
    language: &'a str,
    terms: impl IntoIterator<Item = &'t str>,
) -> impl Future<Output = anyhow::Result<HashMap<String, String>>> + Send + 'a {
    let mut keys: Vec<String> = terms
        .into_iter()
        .map(word_key)
        .filter(|key| !key.is_empty())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    words_of(db, language, keys)
}

async fn words_of(
    db: &Db,
    language: &str,
    keys: Vec<String>,
) -> anyhow::Result<HashMap<String, String>> {
    let mut words = HashMap::with_capacity(keys.len());
    for chunk in keys.chunks(WORDS_CHUNK) {
        let ids: Vec<String> = chunk.iter().map(|key| row_id(language, key)).collect();
        let rows: Vec<(String, String)> = db
            .fetch_all(
                &format!(
                    "SELECT id, wording FROM storefront_words WHERE id IN ({})",
                    placeholders(ids.len())
                ),
                ids,
            )
            .await?;
        for (id, wording) in rows {
            if let Some((_, key)) = id.split_once('\u{1f}') {
                words.insert(key.to_owned(), wording);
            }
        }
    }
    Ok(words)
}

/// `term` as the language of the request reads it: the shop's wording for
/// it there, else the word itself. For a word a page shows on its own (a
/// filter's name); the products, the values a filter offers and the cart
/// the library's readers return are already worded.
pub async fn worded(cx: &Cx, term: &str) -> Result<String> {
    let Some(language) = crate::read_in(cx) else {
        return Ok(term.to_owned());
    };
    let mut words = published_words(store(cx).db(), language, [term])
        .await
        .map_err(Error::from_anyhow)?;
    Ok(words
        .remove(&word_key(term))
        .unwrap_or_else(|| term.to_owned()))
}

/// Copies a word as it stands, and drops the row of one taken back.
#[evento::subscription_all]
pub(crate) async fn on_word_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Word>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = word_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let id = row_id(&state.language, &state.key());
    if state.wording.is_empty() {
        db.execute("DELETE FROM storefront_words WHERE id = ?1", (id,))
            .await?;
        return Ok(());
    }
    db.execute(
        "INSERT INTO storefront_words (id, wording) VALUES (?1, ?2)
         ON CONFLICT (id) DO UPDATE SET wording = excluded.wording",
        (id, state.wording.as_str()),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> Db {
        let db = Db::from(
            timada_core::db::Turso::in_memory_concurrent()
                .await
                .unwrap(),
        );
        crate::migrate(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn the_words_of_a_language_are_read_by_their_key() {
        let db = db().await;
        for (language, key, wording) in [
            ("en", "couleur", "Colour"),
            ("en", "rose foncé", "Dark pink"),
            ("de", "couleur", "Farbe"),
        ] {
            db.execute(
                "INSERT INTO storefront_words (id, wording) VALUES (?1, ?2)",
                (row_id(language, key), wording),
            )
            .await
            .unwrap();
        }
        // Case and the space around a word aside, and each language its own.
        let words = published_words(&db, "en", [" Couleur", "ROSE FONCÉ", "Taille", ""])
            .await
            .unwrap();
        assert_eq!(
            words,
            HashMap::from([
                ("couleur".to_owned(), "Colour".to_owned()),
                ("rose foncé".to_owned(), "Dark pink".to_owned()),
            ])
        );
        assert!(
            published_words(&db, "es", ["Couleur"])
                .await
                .unwrap()
                .is_empty()
        );

        // A seek a word, not a pass over the table.
        let plan: Vec<(i64, i64, i64, String)> = db
            .fetch_all(
                "EXPLAIN QUERY PLAN SELECT id, wording FROM storefront_words WHERE id IN (?1, ?2)",
                ("a", "b"),
            )
            .await
            .unwrap();
        assert!(
            plan.iter()
                .all(|(_, _, _, step)| step.starts_with("SEARCH")),
            "{plan:?}"
        );
    }
}
