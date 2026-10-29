//! The text of the shop's own pages (legal notice, terms of sale, about
//! us), as the merchant published it in the back office (`timada-pages`).
//! The host decides which pages it has and where (`/mentions-legales`), and
//! each of its routes asks for its text by slug with [`page`]. The catalog
//! subscription copies each published page into `storefront_pages`, and
//! drops it once unpublished or deleted. The body is kept
//! as Markdown and turned into HTML on each read, with the pictures served
//! from [`StoreConfig::media_url`](crate::StoreConfig::media_url).

use evento::{Executor, metadata::RawEvent, subscription::Context};
use sqlx::{FromRow, SqlitePool};
use timada_pages::aggregator::Page;
use timada_pages::{PageState, markdown_text, page_projection, render_markdown};
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};
use topcoat_router::error::not_found;

use crate::context::store;
use crate::query::search_key;

/// The longest description a page gives search engines when the merchant
/// wrote none: the start of its text.
const DESCRIPTION_CHARS: usize = 160;

/// A published page, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPage {
    pub id: String,
    pub slug: String,
    pub title: String,
    /// The body as HTML, cleaned: safe to write into the page unescaped.
    pub html: String,
    /// The title search engines show (the page's own unless one was set).
    pub meta_title: String,
    /// The description search engines show: the merchant's, or the start of
    /// the text.
    pub meta_description: String,
}

/// A link to a published page: what a search lists.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct PageLink {
    pub slug: String,
    pub title: String,
}

#[derive(FromRow)]
struct PageRow {
    id: String,
    slug: String,
    title: String,
    body: String,
    text: String,
    meta_title: String,
    meta_description: String,
}

/// A published page by slug, if any.
pub async fn published_page(
    pool: &SqlitePool,
    slug: &str,
    media_url: Option<&str>,
) -> sqlx::Result<Option<PublicPage>> {
    // A slug moving from one page to another is briefly on both: the one
    // that moved last wins.
    let row: Option<PageRow> = sqlx::query_as(
        "SELECT id, slug, title, body, text, meta_title, meta_description
         FROM storefront_pages WHERE slug = ?1 ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| PublicPage {
        html: render_markdown(&row.body, media_url.unwrap_or_default()),
        meta_title: if row.meta_title.is_empty() {
            row.title.clone()
        } else {
            row.meta_title
        },
        meta_description: if row.meta_description.is_empty() {
            excerpt(&row.text, DESCRIPTION_CHARS)
        } else {
            row.meta_description
        },
        id: row.id,
        slug: row.slug,
        title: row.title,
    }))
}

/// The published pages a word of whose title or text starts with `q`,
/// accents and case aside ("livrais" finds "livraison", "lit" does not find
/// "politique"), title matches first, at most `limit`.
pub async fn search_published_pages(
    pool: &SqlitePool,
    q: &str,
    limit: usize,
) -> sqlx::Result<Vec<PageLink>> {
    let key = search_key(q);
    if key.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    // A shop has a handful of pages: they are matched here rather than in
    // SQL, which has no accent folding.
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT slug, title, text FROM storefront_pages ORDER BY title COLLATE NOCASE, id",
    )
    .fetch_all(pool)
    .await?;
    let starts_a_word = |text: &str| {
        search_key(text).split(' ').any(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .starts_with(&key)
        })
    };
    let (mut by_title, mut by_text) = (Vec::new(), Vec::new());
    for (slug, title, text) in rows {
        if starts_a_word(&title) {
            by_title.push(PageLink { slug, title });
        } else if starts_a_word(&text) {
            by_text.push(PageLink { slug, title });
        }
    }
    by_title.extend(by_text);
    by_title.truncate(limit);
    Ok(by_title)
}

/// A published page by slug, if any.
pub async fn page(cx: &Cx, slug: &str) -> Result<Option<PublicPage>> {
    let store = store(cx);
    published_page(store.pool(), slug, store.media_url())
        .await
        .map_err(|err| Error::from_anyhow(err.into()))
}

/// A published page by slug, or 404.
pub async fn require_page(cx: &Cx, slug: &str) -> Result<PublicPage> {
    page(cx, slug).await?.ok_or_else(|| not_found().into())
}

/// The pages a search finds; see [`search_published_pages`].
pub async fn search_pages(cx: &Cx, q: &str, limit: usize) -> Result<Vec<PageLink>> {
    search_published_pages(store(cx).pool(), q, limit)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))
}

/// Copies a page that is published, and drops one that is not (any more).
#[evento::subscription_all]
pub(crate) async fn on_page_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Page>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(page) = page_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_page(&pool, &page).await
}

async fn upsert_page(pool: &SqlitePool, page: &PageState) -> anyhow::Result<()> {
    if !page.published || page.deleted {
        sqlx::query("DELETE FROM storefront_pages WHERE id = ?1")
            .bind(&page.id)
            .execute(pool)
            .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO storefront_pages (id, slug, title, body, text, meta_title,
            meta_description, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
            slug = excluded.slug, title = excluded.title, body = excluded.body,
            text = excluded.text, meta_title = excluded.meta_title,
            meta_description = excluded.meta_description, updated_at = excluded.updated_at",
    )
    .bind(&page.id)
    .bind(&page.slug)
    .bind(&page.title)
    .bind(&page.body)
    .bind(markdown_text(&page.body))
    .bind(&page.meta_title)
    .bind(&page.meta_description)
    .bind(page.updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// The first `max` characters of `text`, cut at a word and marked as cut.
fn excerpt(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max - 1).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.']))
}

#[cfg(test)]
mod tests {
    use super::excerpt;

    #[test]
    fn an_excerpt_ends_at_a_word() {
        assert_eq!(excerpt("short", 10), "short");
        assert_eq!(excerpt("one two, three four", 12), "one two…");
    }
}
