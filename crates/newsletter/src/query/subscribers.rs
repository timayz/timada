//! Queries over `newsletter_subscribers`, for the newsletter page of a
//! tenant.

use timada_core::db::{Db, FromRow};

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
    /// The IANA time zone their browser gave, when it did.
    pub time_zone: Option<String>,
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
    db: &Db,
    tenant_id: &str,
    filter: &SubscriberFilter,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<SubscriberRow>> {
    db.fetch_all(
        "SELECT * FROM newsletter_subscribers
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
         ORDER BY requested_at DESC, id DESC
         LIMIT ?3 OFFSET ?4",
        (
            tenant_id,
            filter.status.map(SubscriberStatus::as_str),
            i64::from(limit),
            i64::from(offset),
        ),
    )
    .await
}

/// How many of the tenant's subscribers match `filter`.
pub async fn count_subscribers(
    db: &Db,
    tenant_id: &str,
    filter: &SubscriberFilter,
) -> anyhow::Result<u64> {
    let (count,): (i64,) = db
        .fetch_one(
            "SELECT COUNT(*) FROM newsletter_subscribers
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)",
            (tenant_id, filter.status.map(SubscriberStatus::as_str)),
        )
        .await?;
    Ok(count as u64)
}
