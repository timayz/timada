//! Queries over `newsletter_campaigns` and `newsletter_deliveries`: the
//! campaigns page of a tenant, and what the sending still owes.

use sqlx::{FromRow, SqlitePool};

use crate::query::SubscriberRow;

/// One row of `newsletter_campaigns`, with what its deliveries count.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct CampaignRow {
    pub tenant_id: String,
    pub id: String,
    pub subject: String,
    pub language: Option<String>,
    pub status: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds, once sent.
    pub sent_at: Option<i64>,
    /// Unix seconds, once every subscriber was mailed.
    pub finished_at: Option<i64>,
    pub updated_at: i64,
    /// Mails handed to the mail server.
    pub delivered: i64,
    /// Mails the mail server did not take.
    pub failed: i64,
}

/// A select of campaigns with their counts, then `$tail` (literal SQL, so
/// the queries stay literals).
macro_rules! select_campaigns {
    ($tail:literal) => {
        concat!(
            "SELECT c.*,
                (SELECT COUNT(*) FROM newsletter_deliveries d
                 WHERE d.tenant_id = c.tenant_id AND d.campaign_id = c.id AND d.failed = 0)
                 AS delivered,
                (SELECT COUNT(*) FROM newsletter_deliveries d
                 WHERE d.tenant_id = c.tenant_id AND d.campaign_id = c.id AND d.failed = 1)
                 AS failed
             FROM newsletter_campaigns c ",
            $tail
        )
    };
}

/// The tenant's campaigns, newest first.
pub async fn campaigns_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<CampaignRow>> {
    Ok(sqlx::query_as(select_campaigns!(
        "WHERE c.tenant_id = ?1
         ORDER BY c.created_at DESC, c.id DESC
         LIMIT ?2 OFFSET ?3"
    ))
    .bind(tenant_id)
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await?)
}

/// How many campaigns the tenant has.
pub async fn count_campaigns(pool: &SqlitePool, tenant_id: &str) -> anyhow::Result<u64> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM newsletter_campaigns WHERE tenant_id = ?1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await?;
    Ok(count as u64)
}

/// One of the tenant's campaigns.
pub async fn campaign_row(
    pool: &SqlitePool,
    tenant_id: &str,
    campaign_id: &str,
) -> anyhow::Result<Option<CampaignRow>> {
    Ok(
        sqlx::query_as(select_campaigns!("WHERE c.tenant_id = ?1 AND c.id = ?2"))
            .bind(tenant_id)
            .bind(campaign_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// The campaigns going out, every tenant's: `(tenant id, campaign id)`,
/// oldest first.
pub async fn sending_campaigns(pool: &SqlitePool) -> anyhow::Result<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT tenant_id, id FROM newsletter_campaigns
         WHERE status = 'sending'
         ORDER BY sent_at, id",
    )
    .fetch_all(pool)
    .await?)
}

/// Up to `limit` confirmed subscribers (in `language`, when given) that the
/// campaign was not mailed to yet, by id. The read model is a moment
/// behind: one claimed a moment ago may come back, and its claim is then
/// refused.
pub async fn undelivered_subscribers(
    pool: &SqlitePool,
    tenant_id: &str,
    campaign_id: &str,
    language: Option<&str>,
    limit: u32,
) -> anyhow::Result<Vec<SubscriberRow>> {
    Ok(sqlx::query_as(
        "SELECT s.* FROM newsletter_subscribers s
         WHERE s.tenant_id = ?1 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d
               WHERE d.tenant_id = s.tenant_id AND d.campaign_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?4",
    )
    .bind(tenant_id)
    .bind(campaign_id)
    .bind(language)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?)
}

/// How many confirmed subscribers (in `language`, when given) a campaign
/// would go to.
pub async fn count_audience(
    pool: &SqlitePool,
    tenant_id: &str,
    language: Option<&str>,
) -> anyhow::Result<u64> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM newsletter_subscribers
         WHERE tenant_id = ?1 AND status = 'confirmed' AND (?2 IS NULL OR language = ?2)",
    )
    .bind(tenant_id)
    .bind(language)
    .fetch_one(pool)
    .await?;
    Ok(count as u64)
}
