//! The subscription that keeps `pages` current for every tenant.
//!
//! The handler reloads the strict write model of the page and rewrites its
//! row, so delivery may repeat (the subscription is at-least-once) without
//! the row drifting; a deleted page loses its row. One subscription follows
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
use crate::command::{PageState, page_projection};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-pages/read-models";

/// Builds the subscription over every tenant and routing key. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_page_event())
        .any_routing_key()
        .retry(u8::MAX)
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
pub async fn delete_page_row(db: &Db, tenant_id: &str, page_id: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM pages WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, page_id),
    )
    .await?;
    Ok(())
}

/// Writes the row of a page as the write model sees it.
pub async fn upsert_page_row(db: &Db, tenant_id: &str, page: &PageState) -> anyhow::Result<()> {
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
