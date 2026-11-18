//! What the shop's products and categories say in its other languages, as
//! the merchant worded them in the back office (`timada-catalog`): one row
//! per thing and language in `storefront_translations`, which the catalog
//! subscription keeps. Its pages and its articles (`timada-pages`) have
//! theirs in `storefront_text_translations`, kept the same way. A thing with no row in a language is read in the
//! shop's main one, and so is each part a row leaves empty.
//!
//! The rows follow the translations alone, whatever the thing they word is
//! (a draft's wording is copied too): a reader joins them to the things it
//! lists, so a row of an unlisted thing is never shown.

use std::collections::HashMap;

use evento::{Executor, metadata::RawEvent, subscription::Context};
pub use timada_catalog::TranslationKind;
use timada_catalog::aggregator::translation::Translation as Stream;
use timada_catalog::translation_projection;
use timada_core::db::{Db, FromRow};
pub use timada_pages::TextKind;
use timada_pages::aggregator::translation::Translation as TextStream;
use timada_pages::text_translation_projection;

use crate::query::{CategoryQuestion, search_key};
use crate::redirects::write_former_slugs;

/// What a product or a category says in one language. Each part is as the
/// merchant wrote it, empty where the main language's stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    pub name: String,
    /// The slug its address goes by in this language.
    pub slug: String,
    pub description: String,
    /// The buying guide below a category's products.
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
    /// The questions a category's page answers.
    pub questions: Vec<CategoryQuestion>,
    /// What a product's pictures show, by picture id, for the ones said in
    /// this language.
    pub alts: HashMap<String, String>,
}

#[derive(FromRow)]
struct Row {
    name: String,
    slug: String,
    description: String,
    body: String,
    meta_title: String,
    meta_description: String,
    questions_json: String,
    alts_json: String,
}

impl From<Row> for Translation {
    fn from(row: Row) -> Self {
        Translation {
            name: row.name,
            slug: row.slug,
            description: row.description,
            body: row.body,
            meta_title: row.meta_title,
            meta_description: row.meta_description,
            questions: serde_json::from_str(&row.questions_json).unwrap_or_default(),
            alts: serde_json::from_str(&row.alts_json).unwrap_or_default(),
        }
    }
}

/// The key of `storefront_translations`, named wherever a statement reads
/// a thing's rows by it: left to itself the planner takes the slug's
/// index, led by `kind` as the key is, and reads every row of the kind and
/// the language for each thing looked up.
pub(crate) const TRANSLATIONS_KEY: &str = "sqlite_autoindex_storefront_translations_1";
/// The same for `storefront_text_translations`.
pub(crate) const TEXT_TRANSLATIONS_KEY: &str = "sqlite_autoindex_storefront_text_translations_1";

const COLUMNS: &str =
    "name, slug, description, body, meta_title, meta_description, questions_json, alts_json";

/// What the thing says in `language`; `None` where it has no translation.
pub async fn translation_of(
    db: &Db,
    kind: TranslationKind,
    subject_id: &str,
    language: &str,
) -> anyhow::Result<Option<Translation>> {
    let row: Option<Row> = db
        .fetch_optional(
            &format!(
                "SELECT {COLUMNS} FROM storefront_translations INDEXED BY {TRANSLATIONS_KEY}
                 WHERE kind = ?1 AND subject_id = ?2 AND language = ?3"
            ),
            (kind.as_str(), subject_id, language),
        )
        .await?;
    Ok(row.map(Translation::from))
}

/// The id of the thing whose address in `language` goes by `slug`, as a
/// translation says; `None` when no translation holds it (a thing with none
/// goes by its main slug there).
pub async fn translated_slug(
    db: &Db,
    kind: TranslationKind,
    language: &str,
    slug: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = db
        .fetch_optional(
            "SELECT subject_id FROM storefront_translations
             WHERE kind = ?1 AND language = ?2 AND slug = ?3
             ORDER BY subject_id LIMIT 1",
            (kind.as_str(), language, slug),
        )
        .await?;
    Ok(row.map(|(id,)| id))
}

/// Copies a translation as it stands, with the slugs it left, and drops
/// the row of one that was dropped.
#[evento::subscription_all]
pub(crate) async fn on_translation_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Stream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = translation_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    // The slugs it left in this language, a dropped one's last among
    // them: where an old address there is redirected from.
    db.write(&state, |tx, state| {
        Box::pin(async move {
            write_former_slugs(
                tx,
                &state.kind,
                &state.language,
                &state.subject_id,
                &state.former_slugs,
            )
            .await
        })
    })
    .await?;
    if state.dropped {
        db.execute(
            &format!(
                "DELETE FROM storefront_translations INDEXED BY {TRANSLATIONS_KEY}
                 WHERE kind = ?1 AND subject_id = ?2 AND language = ?3"
            ),
            (
                state.kind.as_str(),
                state.subject_id.as_str(),
                state.language.as_str(),
            ),
        )
        .await?;
        return Ok(());
    }
    let questions: Vec<CategoryQuestion> = state
        .questions
        .iter()
        .map(|asked| CategoryQuestion {
            question: asked.question.clone(),
            answer: asked.answer.clone(),
        })
        .collect();
    let alts: HashMap<&str, &str> = state
        .alts
        .iter()
        .map(|said| (said.image_id.as_str(), said.alt.as_str()))
        .collect();
    db.execute(
        "INSERT INTO storefront_translations
            (kind, subject_id, language, name, slug, description, body, meta_title,
             meta_description, questions_json, slug_search, name_search, alts_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT (kind, subject_id, language) DO UPDATE SET
            name = excluded.name, slug = excluded.slug, description = excluded.description,
            body = excluded.body, meta_title = excluded.meta_title,
            meta_description = excluded.meta_description,
            questions_json = excluded.questions_json, alts_json = excluded.alts_json,
            slug_search = excluded.slug_search, name_search = excluded.name_search",
        (
            state.kind.as_str(),
            state.subject_id.as_str(),
            state.language.as_str(),
            state.name.as_str(),
            state.slug.as_str(),
            state.description.as_str(),
            state.body.as_str(),
            state.meta_title.as_str(),
            state.meta_description.as_str(),
            serde_json::to_string(&questions)?,
            // What a search in this language finds it by.
            search_key(&state.slug),
            search_key(&state.name),
            serde_json::to_string(&alts)?,
        ),
    )
    .await?;
    Ok(())
}

/// What a page or an article says in one language. Each part is as the
/// merchant wrote it, empty where the main language's stands.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct TextTranslation {
    pub title: String,
    /// The slug an article's address goes by in this language; empty for a
    /// page, whose address is the host's.
    pub slug: String,
    /// An article's summary.
    pub summary: String,
    /// Markdown, as written.
    pub body: String,
    pub meta_title: String,
    pub meta_description: String,
}

/// What the page or the article says in `language`; `None` where it has no
/// translation.
pub async fn text_translation_of(
    db: &Db,
    kind: TextKind,
    subject_id: &str,
    language: &str,
) -> anyhow::Result<Option<TextTranslation>> {
    db.fetch_optional(
        &format!(
            "SELECT title, slug, summary, body, meta_title, meta_description
             FROM storefront_text_translations INDEXED BY {TEXT_TRANSLATIONS_KEY}
             WHERE kind = ?1 AND subject_id = ?2 AND language = ?3"
        ),
        (kind.as_str(), subject_id, language),
    )
    .await
}

/// The id of the article whose address in `language` goes by `slug`, as a
/// translation says; `None` when no translation holds it (an article with
/// none goes by its main slug there).
pub async fn translated_article_slug(
    db: &Db,
    language: &str,
    slug: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = db
        .fetch_optional(
            "SELECT subject_id FROM storefront_text_translations
             WHERE kind = 'article' AND language = ?1 AND slug = ?2
             ORDER BY subject_id LIMIT 1",
            (language, slug),
        )
        .await?;
    Ok(row.map(|(id,)| id))
}

/// Copies a page's or an article's translation as it stands, and drops the
/// row of one that was dropped.
#[evento::subscription_all]
pub(crate) async fn on_text_translation_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<TextStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = text_translation_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    // A page has no slug of its own in a language; an article does, and
    // leaves it as a product does.
    if state.kind == TextKind::Article.as_str() {
        db.write(&state, |tx, state| {
            Box::pin(async move {
                write_former_slugs(
                    tx,
                    &state.kind,
                    &state.language,
                    &state.subject_id,
                    &state.former_slugs,
                )
                .await
            })
        })
        .await?;
    }
    if state.dropped {
        db.execute(
            &format!(
                "DELETE FROM storefront_text_translations INDEXED BY {TEXT_TRANSLATIONS_KEY}
                 WHERE kind = ?1 AND subject_id = ?2 AND language = ?3"
            ),
            (
                state.kind.as_str(),
                state.subject_id.as_str(),
                state.language.as_str(),
            ),
        )
        .await?;
        return Ok(());
    }
    db.execute(
        "INSERT INTO storefront_text_translations
            (kind, subject_id, language, title, slug, summary, body, meta_title,
             meta_description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (kind, subject_id, language) DO UPDATE SET
            title = excluded.title, slug = excluded.slug, summary = excluded.summary,
            body = excluded.body, meta_title = excluded.meta_title,
            meta_description = excluded.meta_description",
        (
            state.kind.as_str(),
            state.subject_id.as_str(),
            state.language.as_str(),
            state.title.as_str(),
            state.slug.as_str(),
            state.summary.as_str(),
            state.body.as_str(),
            state.meta_title.as_str(),
            state.meta_description.as_str(),
        ),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thing's translation is read by its key, one row, not by a pass
    /// over every row of its kind and language: the plans of the lookups
    /// the readers make for each product of a listing.
    #[tokio::test]
    async fn a_things_translation_is_read_by_its_key() {
        let db = Db::from(
            timada_core::db::Turso::in_memory_concurrent()
                .await
                .unwrap(),
        );
        crate::migrate(&db).await.unwrap();
        let slug_of = crate::sitemap::slug_in("storefront_translations", "product", "p", 1);
        for (sql, binds, key, columns) in [
            (
                format!(
                    "SELECT {COLUMNS} FROM storefront_translations INDEXED BY {TRANSLATIONS_KEY}
                     WHERE kind = ?1 AND subject_id = ?2 AND language = ?3"
                ),
                3,
                TRANSLATIONS_KEY,
                "kind=? AND subject_id=? AND language=?",
            ),
            (
                format!(
                    "SELECT title FROM storefront_text_translations
                        INDEXED BY {TEXT_TRANSLATIONS_KEY}
                     WHERE kind = ?1 AND subject_id = ?2 AND language = ?3"
                ),
                3,
                TEXT_TRANSLATIONS_KEY,
                "kind=? AND subject_id=? AND language=?",
            ),
            (
                format!(
                    "DELETE FROM storefront_translations INDEXED BY {TRANSLATIONS_KEY}
                     WHERE kind = ?1 AND subject_id = ?2 AND language = ?3"
                ),
                3,
                TRANSLATIONS_KEY,
                "kind=? AND subject_id=? AND language=?",
            ),
            (
                format!(
                    "SELECT language, slug FROM storefront_translations
                        INDEXED BY {TRANSLATIONS_KEY}
                     WHERE kind = ?1 AND subject_id = ?2"
                ),
                2,
                TRANSLATIONS_KEY,
                "kind=? AND subject_id=?",
            ),
            (
                format!("SELECT {slug_of} FROM storefront_products p WHERE p.id = ?2"),
                2,
                TRANSLATIONS_KEY,
                "kind=? AND subject_id=? AND language=?",
            ),
        ] {
            let plan: Vec<(i64, i64, i64, String)> = db
                .fetch_all(
                    &format!("EXPLAIN QUERY PLAN {sql}"),
                    timada_core::db::params_from_iter(
                        ["product", "a", "en"].into_iter().take(binds),
                    ),
                )
                .await
                .unwrap();
            assert!(
                plan.iter().any(|(_, _, _, step)| step.starts_with("SEARCH")
                    && step.contains(&format!("{key} ({columns})"))),
                "{sql}: {plan:?}"
            );
        }
    }
}
