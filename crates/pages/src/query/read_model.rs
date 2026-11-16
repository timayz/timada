//! The subscription that keeps `pages` and `articles` current for every
//! tenant.
//!
//! A handler reloads the strict write model of the page or the article and
//! rewrites its row, so delivery may repeat (the subscription is
//! at-least-once) without the row drifting; a deleted one loses its row. One subscription follows
//! every tenant's streams (its cursor lives in the store): each row carries
//! the tenant of the event that made it, and the rows live in the store's
//! own database ([`Db`]).

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::Db;

use crate::aggregator::Page;
use crate::aggregator::article::Article;
use crate::command::{ArticleState, PageState, article_projection, page_projection};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-pages/read-models";

/// Builds the subscription over every tenant and routing key. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_page_event())
        .handler(on_article_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_page_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Page>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(page) = page_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if page.deleted {
        return delete_page_row(&db, &event.tenant, &page.id).await;
    }
    upsert_page_row(&db, &event.tenant, &page).await
}

/// Drops the row of a deleted page.
pub(crate) async fn delete_page_row(db: &Db, tenant_id: &str, page_id: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM pages WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, page_id),
    )
    .await?;
    Ok(())
}

/// Writes the row of a page as the write model sees it.
pub(crate) async fn upsert_page_row(
    db: &Db,
    tenant_id: &str,
    page: &PageState,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO pages (tenant_id, id, slug, title, published, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            slug = excluded.slug, title = excluded.title, published = excluded.published,
            updated_at = excluded.updated_at",
        (
            tenant_id,
            page.id.as_str(),
            page.slug.as_str(),
            page.title.as_str(),
            page.published,
            page.created_at as i64,
            page.updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}

#[evento::subscription_all]
async fn on_article_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Article>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(article) = article_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if article.deleted {
        db.execute(
            "DELETE FROM articles WHERE tenant_id = ?1 AND id = ?2",
            (event.tenant.as_str(), article.id.as_str()),
        )
        .await?;
        return Ok(());
    }
    upsert_article_row(&db, &event.tenant, &article).await
}

/// Writes the row of an article as the write model sees it.
pub(crate) async fn upsert_article_row(
    db: &Db,
    tenant_id: &str,
    article: &ArticleState,
) -> anyhow::Result<()> {
    let listed_at = if article.published_at == 0 {
        article.created_at
    } else {
        article.published_at
    };
    db.execute(
        "INSERT INTO articles (tenant_id, id, slug, title, published, published_at, listed_at,
            updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            slug = excluded.slug, title = excluded.title, published = excluded.published,
            published_at = excluded.published_at, listed_at = excluded.listed_at,
            updated_at = excluded.updated_at",
        (
            tenant_id,
            article.id.as_str(),
            article.slug.as_str(),
            article.title.as_str(),
            article.published,
            article.published_at as i64,
            listed_at as i64,
            article.updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}
