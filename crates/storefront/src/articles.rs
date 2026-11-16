//! The shop's blog, as the merchant published it in the back office
//! (`timada-pages`). The host decides where it is (`/blog`, `/blog/{slug}`)
//! and how it looks; [`articles`] gives a page of its list, the most recent
//! first, and [`article`] one of them, its body cut where it shows a
//! product or a category ([`ArticleBlock`]) and its headings listed for a
//! table of contents. The catalog subscription copies each published
//! article into `storefront_articles`, and drops it once unpublished or
//! deleted. The body is kept as Markdown and turned into HTML on each read,
//! with the pictures served from
//! [`StoreConfig::media_url`](crate::StoreConfig::media_url).

use evento::{Executor, metadata::RawEvent, subscription::Context};
use timada_core::db::{Db, FromRow, execute};
use timada_pages::aggregator::article::Article;
pub use timada_pages::{ArticleBlock, ArticleHeading};
use timada_pages::{ArticleState, article_projection, article_text, render_article};
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};
use topcoat_router::error::not_found;

use crate::context::store;
use crate::pages::{DESCRIPTION_CHARS, excerpt};
use crate::query::Image;
use crate::redirects::{ARTICLE, write_former_slugs};

/// How long the summary of an article that has none is: the start of its
/// text.
const SUMMARY_CHARS: usize = 240;

/// An article in the blog's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleCard {
    pub slug: String,
    pub title: String,
    /// The merchant's summary, or the start of the text.
    pub summary: String,
    pub cover: Option<Image>,
    /// The day it came out, in Unix seconds.
    pub published_at: i64,
}

/// A page of the blog's list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArticlePage {
    pub articles: Vec<ArticleCard>,
    /// From 1.
    pub page: u32,
    /// How many pages the list has: 0 for a blog with nothing published.
    pub pages: u32,
}

/// A published article, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicArticle {
    pub id: String,
    pub slug: String,
    pub title: String,
    /// The merchant's summary; may be empty.
    pub summary: String,
    /// The body in order: text, and the products and categories it shows.
    pub blocks: Vec<ArticleBlock>,
    /// The body's `##` and `###` headings, for a table of contents.
    pub headings: Vec<ArticleHeading>,
    /// The title search engines show (the article's own unless one was set).
    pub meta_title: String,
    /// The description search engines show: the merchant's, the summary, or
    /// the start of the text.
    pub meta_description: String,
    pub cover: Option<Image>,
    /// The day it came out, in Unix seconds.
    pub published_at: i64,
    /// When it last changed, in Unix seconds.
    pub updated_at: i64,
}

#[derive(FromRow)]
struct CardRow {
    slug: String,
    title: String,
    summary: String,
    excerpt: String,
    cover_json: Option<String>,
    published_at: i64,
}

#[derive(FromRow)]
struct ArticleRow {
    id: String,
    slug: String,
    title: String,
    summary: String,
    body: String,
    excerpt: String,
    meta_title: String,
    meta_description: String,
    cover_json: Option<String>,
    published_at: i64,
    updated_at: i64,
}

/// The cover a row keeps, pointed at the store's media address.
fn cover_of(json: Option<&str>, media_url: Option<&str>) -> Option<Image> {
    let mut cover: Image = serde_json::from_str(json?).ok()?;
    cover.fill_urls(media_url);
    Some(cover)
}

/// Page `page` (from 1) of the published articles, the most recent first,
/// `per_page` each. A page past the last is empty.
pub async fn published_articles(
    db: &Db,
    page: u32,
    per_page: u32,
    media_url: Option<&str>,
) -> anyhow::Result<ArticlePage> {
    let page = page.max(1);
    let per_page = per_page.max(1);
    let total: i64 = db
        .fetch_scalar("SELECT COUNT(*) FROM storefront_articles", ())
        .await?;
    let rows: Vec<CardRow> = db
        .fetch_all(
            "SELECT slug, title, summary, excerpt, cover_json, published_at
             FROM storefront_articles INDEXED BY storefront_articles_published
             ORDER BY published_at DESC, id DESC LIMIT ?1 OFFSET ?2",
            (
                i64::from(per_page),
                i64::from(page - 1) * i64::from(per_page),
            ),
        )
        .await?;
    Ok(ArticlePage {
        articles: rows
            .into_iter()
            .map(|row| ArticleCard {
                cover: cover_of(row.cover_json.as_deref(), media_url),
                summary: if row.summary.is_empty() {
                    row.excerpt
                } else {
                    row.summary
                },
                slug: row.slug,
                title: row.title,
                published_at: row.published_at,
            })
            .collect(),
        page,
        pages: (total.max(0) as u64).div_ceil(u64::from(per_page)) as u32,
    })
}

/// Whether the blog has anything published: whether a link to it leads
/// somewhere.
pub async fn any_published_article(db: &Db) -> anyhow::Result<bool> {
    let any: Option<(i64,)> = db
        .fetch_optional("SELECT 1 FROM storefront_articles LIMIT 1", ())
        .await?;
    Ok(any.is_some())
}

/// A published article by slug, if any.
pub async fn published_article(
    db: &Db,
    slug: &str,
    media_url: Option<&str>,
) -> anyhow::Result<Option<PublicArticle>> {
    // A slug moving from one article to another is briefly on both: the
    // one that moved last wins.
    let row: Option<ArticleRow> = db
        .fetch_optional(
            "SELECT id, slug, title, summary, body, excerpt, meta_title, meta_description,
                cover_json, published_at, updated_at
             FROM storefront_articles WHERE slug = ?1 ORDER BY updated_at DESC LIMIT 1",
            (slug,),
        )
        .await?;
    Ok(row.map(|row| {
        let rendered = render_article(&row.body, media_url.unwrap_or_default());
        PublicArticle {
            blocks: rendered.blocks,
            headings: rendered.headings,
            cover: cover_of(row.cover_json.as_deref(), media_url),
            meta_title: if row.meta_title.is_empty() {
                row.title.clone()
            } else {
                row.meta_title
            },
            meta_description: if !row.meta_description.is_empty() {
                row.meta_description
            } else if !row.summary.is_empty() {
                excerpt(&row.summary, DESCRIPTION_CHARS)
            } else {
                excerpt(&row.excerpt, DESCRIPTION_CHARS)
            },
            id: row.id,
            slug: row.slug,
            title: row.title,
            summary: row.summary,
            published_at: row.published_at,
            updated_at: row.updated_at,
        }
    }))
}

/// A page of the blog's list; see [`published_articles`].
pub async fn articles(cx: &Cx, page: u32, per_page: u32) -> Result<ArticlePage> {
    let store = store(cx);
    published_articles(store.db(), page, per_page, store.media_url())
        .await
        .map_err(Error::from_anyhow)
}

/// Whether the blog has anything published; see [`any_published_article`].
pub async fn has_articles(cx: &Cx) -> Result<bool> {
    any_published_article(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// A published article by slug, if any.
pub async fn article(cx: &Cx, slug: &str) -> Result<Option<PublicArticle>> {
    let store = store(cx);
    published_article(store.db(), slug, store.media_url())
        .await
        .map_err(Error::from_anyhow)
}

/// A published article by slug, or 404.
pub async fn require_article(cx: &Cx, slug: &str) -> Result<PublicArticle> {
    article(cx, slug).await?.ok_or_else(|| not_found().into())
}

/// Copies an article that is published, and drops one that is not (any
/// more).
#[evento::subscription_all]
pub(crate) async fn on_article_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Article>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(article) = article_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_article(&db, &article).await
}

async fn upsert_article(db: &Db, article: &ArticleState) -> anyhow::Result<()> {
    db.write(article, |tx, article| {
        Box::pin(async move {
            if !article.published || article.deleted {
                execute(
                    tx,
                    "DELETE FROM storefront_articles WHERE id = ?1",
                    (article.id.as_str(),),
                )
                .await?;
                // An article nobody reads is not redirected to.
                return write_former_slugs(tx, ARTICLE, &article.id, &[]).await;
            }
            let cover = article.cover_image().map(|image| Image {
                id: image.id.clone(),
                alt: String::new(),
                width: image.picture.page.width,
                height: image.picture.page.height,
                thumb_width: image.picture.listing.width,
                thumb_height: image.picture.listing.height,
                // Filled in when a page reads the article: the address
                // belongs to the host's settings.
                url: String::new(),
                thumb_url: String::new(),
            });
            let cover_json = cover
                .map(|cover| serde_json::to_string(&cover))
                .transpose()?;
            execute(
                tx,
                "INSERT INTO storefront_articles (id, slug, title, summary, body, excerpt,
                    meta_title, meta_description, cover_json, published_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT (id) DO UPDATE SET
                    slug = excluded.slug, title = excluded.title, summary = excluded.summary,
                    body = excluded.body, excerpt = excluded.excerpt,
                    meta_title = excluded.meta_title,
                    meta_description = excluded.meta_description,
                    cover_json = excluded.cover_json, published_at = excluded.published_at,
                    updated_at = excluded.updated_at",
                (
                    article.id.as_str(),
                    article.slug.as_str(),
                    article.title.as_str(),
                    article.summary.as_str(),
                    article.body.as_str(),
                    excerpt(&article_text(&article.body), SUMMARY_CHARS),
                    article.meta_title.as_str(),
                    article.meta_description.as_str(),
                    cover_json,
                    article.published_at as i64,
                    article.updated_at as i64,
                ),
            )
            .await?;
            // The slugs it left: where an old address is redirected from.
            write_former_slugs(tx, ARTICLE, &article.id, &article.former_slugs).await
        })
    })
    .await
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

    fn state(id: &str, slug: &str, published_at: u64) -> ArticleState {
        ArticleState {
            id: id.to_owned(),
            slug: slug.to_owned(),
            title: format!("Titre {slug}"),
            body: "Un *long* texte.\n\n[[product:tapis-bleu]]\n\n## La suite\n\nFin.".to_owned(),
            published: true,
            published_at,
            updated_at: published_at,
            ..ArticleState::default()
        }
    }

    #[tokio::test]
    async fn the_list_is_the_published_articles_most_recent_first() {
        let db = db().await;
        assert!(!any_published_article(&db).await.unwrap());
        for (id, slug, at) in [("a1", "un", 10), ("a2", "deux", 30), ("a3", "trois", 20)] {
            upsert_article(&db, &state(id, slug, at)).await.unwrap();
        }
        let first = published_articles(&db, 1, 2, None).await.unwrap();
        let slugs = |page: &ArticlePage| {
            page.articles
                .iter()
                .map(|card| card.slug.clone())
                .collect::<Vec<_>>()
        };
        assert!(any_published_article(&db).await.unwrap());
        assert_eq!(slugs(&first), ["deux", "trois"]);
        assert_eq!((first.page, first.pages), (1, 2));
        // No summary written: the start of the text, the product left out.
        assert_eq!(first.articles[0].summary, "Un long texte. La suite Fin.");
        assert_eq!(first.articles[0].cover, None);
        let second = published_articles(&db, 2, 2, None).await.unwrap();
        assert_eq!(slugs(&second), ["un"]);
        assert!(
            published_articles(&db, 3, 2, None)
                .await
                .unwrap()
                .articles
                .is_empty()
        );

        // Unpublished: off the list, and its address answers nothing.
        let mut draft = state("a2", "deux", 30);
        draft.published = false;
        upsert_article(&db, &draft).await.unwrap();
        let left = published_articles(&db, 1, 10, None).await.unwrap();
        assert_eq!(slugs(&left), ["trois", "un"]);
        assert_eq!(left.pages, 1);
        assert_eq!(published_article(&db, "deux", None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_article_reads_with_its_blocks_cover_and_what_search_engines_show() {
        let db = db().await;
        let mut article = state("a1", "choisir", 10);
        article.summary = "Comment choisir.".to_owned();
        article.images = vec![timada_pages::PageImage {
            id: "01JBX0V7Y5B6V9H8K3QWERTYAS".to_owned(),
            picture: timada_core::Picture {
                width: 1600,
                height: 1200,
                listing: timada_core::RenditionFile {
                    width: 400,
                    height: 300,
                    size: 1,
                    sha256: [0; 32],
                },
                page: timada_core::RenditionFile {
                    width: 1200,
                    height: 900,
                    size: 1,
                    sha256: [0; 32],
                },
            },
        }];
        article.cover = Some("01JBX0V7Y5B6V9H8K3QWERTYAS".to_owned());
        upsert_article(&db, &article).await.unwrap();

        let read = published_article(&db, "choisir", Some("https://media.example/acme"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read.meta_title, "Titre choisir");
        assert_eq!(read.meta_description, "Comment choisir.");
        assert_eq!(read.blocks.len(), 3);
        assert_eq!(read.blocks[1], ArticleBlock::Product("tapis-bleu".into()));
        assert_eq!(read.headings.len(), 1);
        assert_eq!(read.headings[0].id, "s-la-suite");
        let cover = read.cover.unwrap();
        assert_eq!((cover.width, cover.thumb_width), (1200, 400));
        assert_eq!(
            cover.thumb_url,
            "https://media.example/acme/400/01JBX0V7Y5B6V9H8K3QWERTYAS.webp"
        );
        let card = &published_articles(&db, 1, 10, None).await.unwrap().articles[0];
        assert_eq!(card.summary, "Comment choisir.");
        assert!(card.cover.is_some());
    }

    #[tokio::test]
    async fn the_list_walks_its_index() {
        let db = db().await;
        let plan: Vec<String> = db
            .fetch_all::<(i64, i64, i64, String)>(
                "EXPLAIN QUERY PLAN SELECT slug, title, summary, excerpt, cover_json, published_at
                 FROM storefront_articles INDEXED BY storefront_articles_published
                 ORDER BY published_at DESC, id DESC LIMIT ?1 OFFSET ?2",
                (10, 0),
            )
            .await
            .unwrap()
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect();
        assert!(
            plan.iter().all(|step| !step.contains("TEMP B-TREE")),
            "{plan:#?}"
        );
    }
}
