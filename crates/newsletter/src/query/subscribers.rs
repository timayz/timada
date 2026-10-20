//! Queries over `newsletter_subscribers`, for the newsletter page of a
//! tenant.

use sqlx::{FromRow, SqlitePool};

use crate::value_object::SubscriberStatus;

/// One row of `newsletter_subscribers`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct SubscriberRow {
    pub tenant_id: String,
    pub id: String,
    pub email: String,
    pub language: Option<String>,
    pub source: String,
    pub status: String,
    /// Unix seconds of the last request.
    pub requested_at: i64,
    /// Unix seconds, while confirmed.
    pub confirmed_at: Option<i64>,
    /// Unix seconds, while unsubscribed.
    pub unsubscribed_at: Option<i64>,
    /// Unix seconds.
    pub updated_at: i64,
}

impl SubscriberRow {
    pub fn status(&self) -> SubscriberStatus {
        SubscriberStatus::parse(&self.status).unwrap_or_default()
    }
}

/// Which subscribers a list shows; `None` is any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubscriberFilter {
    pub status: Option<SubscriberStatus>,
}

/// The tenant's subscribers matching `filter`, most recently asked first.
pub async fn subscribers_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &SubscriberFilter,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<SubscriberRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM newsletter_subscribers
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
         ORDER BY requested_at DESC, id DESC
         LIMIT ?3 OFFSET ?4",
    )
    .bind(tenant_id)
    .bind(filter.status.map(SubscriberStatus::as_str))
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await?)
}

/// How many of the tenant's subscribers match `filter`.
pub async fn count_subscribers(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &SubscriberFilter,
) -> anyhow::Result<u64> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM newsletter_subscribers
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)",
    )
    .bind(tenant_id)
    .bind(filter.status.map(SubscriberStatus::as_str))
    .fetch_one(pool)
    .await?;
    Ok(count as u64)
}
