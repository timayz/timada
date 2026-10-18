//! Queries over `support_messages`, for the messages page of a tenant.

use sqlx::{FromRow, SqlitePool};

use crate::value_object::MessageStatus;

/// One row of `support_messages`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct MessageRow {
    pub tenant_id: String,
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub body: String,
    pub language: Option<String>,
    pub status: String,
    /// Unix seconds.
    pub received_at: i64,
    /// Unix seconds, while it is handled.
    pub handled_at: Option<i64>,
    /// Unix seconds.
    pub updated_at: i64,
}

impl MessageRow {
    /// `Ada Lovelace`.
    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
    }
}

/// Which messages a list shows; `None` is any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageFilter {
    pub status: Option<MessageStatus>,
}

/// The tenant's messages matching `filter`, newest first.
pub async fn messages_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &MessageFilter,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<MessageRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM support_messages
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
         ORDER BY received_at DESC, id DESC
         LIMIT ?3 OFFSET ?4",
    )
    .bind(tenant_id)
    .bind(filter.status.map(MessageStatus::as_str))
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await?)
}

/// How many of the tenant's messages match `filter`.
pub async fn count_messages(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &MessageFilter,
) -> anyhow::Result<u64> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM support_messages
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)",
    )
    .bind(tenant_id)
    .bind(filter.status.map(MessageStatus::as_str))
    .fetch_one(pool)
    .await?;
    Ok(count as u64)
}
