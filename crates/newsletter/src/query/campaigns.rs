//! Queries over `newsletter_campaigns`, `newsletter_editions` and
//! `newsletter_deliveries`: the campaigns page of a tenant, and what the
//! sending still owes.
//!
//! Every lookup in `newsletter_deliveries` names its index (`INDEXED BY`):
//! Turso's planner costs each column of the composite primary key as unique
//! on its own and so takes whichever index pins `tenant_id`, then walks
//! every delivery of the tenant once per outer row
//! (tursodatabase/turso#8664). The primary key's index is the one SQLite
//! names for a table's `PRIMARY KEY` constraint. Issue #336 has the plans
//! and timings; `tests/query_plans.rs` runs them again.

use timada_core::db::{Db, FromRow};

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
    /// Subscribers who left the list before their mail went out.
    pub skipped: i64,
    /// The cadence of its schedule, when it has one.
    pub cadence: Option<String>,
}

/// A select of campaigns with their counts, then `$tail` (literal SQL, so
/// the queries stay literals).
macro_rules! select_campaigns {
    ($tail:literal) => {
        concat!(
            "SELECT c.*,
                (SELECT COUNT(*) FROM newsletter_deliveries d INDEXED BY newsletter_deliveries_campaign
                 WHERE d.tenant_id = c.tenant_id AND d.campaign_id = c.id
                   AND d.failed = 0 AND d.skipped = 0)
                 AS delivered,
                (SELECT COUNT(*) FROM newsletter_deliveries d INDEXED BY newsletter_deliveries_campaign
                 WHERE d.tenant_id = c.tenant_id AND d.campaign_id = c.id AND d.failed = 1)
                 AS failed,
                (SELECT COUNT(*) FROM newsletter_deliveries d INDEXED BY newsletter_deliveries_campaign
                 WHERE d.tenant_id = c.tenant_id AND d.campaign_id = c.id AND d.skipped = 1)
                 AS skipped
             FROM newsletter_campaigns c ",
            $tail
        )
    };
}

/// The tenant's campaigns, newest first.
pub async fn campaigns_of_tenant(
    db: &Db,
    tenant_id: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<CampaignRow>> {
    db.fetch_all(
        select_campaigns!(
            "WHERE c.tenant_id = ?1
         ORDER BY c.created_at DESC, c.id DESC
         LIMIT ?2 OFFSET ?3"
        ),
        (tenant_id, i64::from(limit), i64::from(offset)),
    )
    .await
}

/// How many campaigns the tenant has.
pub async fn count_campaigns(db: &Db, tenant_id: &str) -> anyhow::Result<u64> {
    let (count,): (i64,) = db
        .fetch_one(
            "SELECT COUNT(*) FROM newsletter_campaigns WHERE tenant_id = ?1",
            (tenant_id,),
        )
        .await?;
    Ok(count as u64)
}

/// One of the tenant's campaigns.
pub async fn campaign_row(
    db: &Db,
    tenant_id: &str,
    campaign_id: &str,
) -> anyhow::Result<Option<CampaignRow>> {
    db.fetch_optional(
        select_campaigns!("WHERE c.tenant_id = ?1 AND c.id = ?2"),
        (tenant_id, campaign_id),
    )
    .await
}

/// The campaigns going out, every tenant's: `(tenant id, campaign id)`,
/// oldest first.
pub async fn sending_campaigns(db: &Db) -> anyhow::Result<Vec<(String, String)>> {
    db.fetch_all(
        "SELECT tenant_id, id FROM newsletter_campaigns
         WHERE status = 'sending'
         ORDER BY sent_at, id",
        (),
    )
    .await
}

/// Up to `limit` confirmed subscribers (in `language`, when given) that the
/// campaign was not mailed to yet, by id. The read model is a moment
/// behind: one claimed a moment ago may come back, and its claim is then
/// refused.
pub async fn undelivered_subscribers(
    db: &Db,
    tenant_id: &str,
    campaign_id: &str,
    language: Option<&str>,
    limit: u32,
) -> anyhow::Result<Vec<SubscriberRow>> {
    db.fetch_all(
        "SELECT s.* FROM newsletter_subscribers s
         WHERE s.tenant_id = ?1 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d
                 INDEXED BY sqlite_autoindex_newsletter_deliveries_1
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?4",
        (tenant_id, campaign_id, language, i64::from(limit)),
    )
    .await
}

/// How many confirmed subscribers (in `language`, when given) a campaign
/// would go to.
pub async fn count_audience(
    db: &Db,
    tenant_id: &str,
    language: Option<&str>,
) -> anyhow::Result<u64> {
    let (count,): (i64,) = db
        .fetch_one(
            "SELECT COUNT(*) FROM newsletter_subscribers
         WHERE tenant_id = ?1 AND status = 'confirmed' AND (?2 IS NULL OR language = ?2)",
            (tenant_id, language),
        )
        .await?;
    Ok(count as u64)
}

/// The scheduled campaigns, every tenant's: `(tenant id, campaign id)`.
pub async fn scheduled_campaigns(db: &Db) -> anyhow::Result<Vec<(String, String)>> {
    db.fetch_all(
        "SELECT tenant_id, id FROM newsletter_campaigns
         WHERE status = 'scheduled'
         ORDER BY sent_at, id",
        (),
    )
    .await
}

/// The editions going out, every tenant's: `(tenant id, edition id)`.
pub async fn open_editions(db: &Db) -> anyhow::Result<Vec<(String, String)>> {
    db.fetch_all(
        "SELECT tenant_id, id FROM newsletter_editions
         WHERE status = 'open'
         ORDER BY opened_at, id",
        (),
    )
    .await
}

/// One row of `newsletter_editions`, with what its deliveries count.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct EditionRow {
    pub tenant_id: String,
    pub id: String,
    pub campaign_id: String,
    /// The first day of the cycle it goes out in, `YYYY-MM-DD`.
    pub cycle: String,
    /// `open` or `closed`.
    pub status: String,
    /// Unix seconds.
    pub opened_at: i64,
    pub closed_at: Option<i64>,
    pub delivered: i64,
    pub failed: i64,
    /// Subscribers who left the list before their mail went out.
    pub skipped: i64,
}

/// A campaign's editions, newest first.
pub async fn editions_of_campaign(
    db: &Db,
    tenant_id: &str,
    campaign_id: &str,
    limit: u32,
) -> anyhow::Result<Vec<EditionRow>> {
    db.fetch_all(
        "SELECT e.*,
            (SELECT COUNT(*) FROM newsletter_deliveries d
                INDEXED BY sqlite_autoindex_newsletter_deliveries_1
             WHERE d.tenant_id = e.tenant_id AND d.edition_id = e.id
               AND d.failed = 0 AND d.skipped = 0)
             AS delivered,
            (SELECT COUNT(*) FROM newsletter_deliveries d
                INDEXED BY sqlite_autoindex_newsletter_deliveries_1
             WHERE d.tenant_id = e.tenant_id AND d.edition_id = e.id AND d.failed = 1)
             AS failed,
            (SELECT COUNT(*) FROM newsletter_deliveries d
                INDEXED BY sqlite_autoindex_newsletter_deliveries_1
             WHERE d.tenant_id = e.tenant_id AND d.edition_id = e.id AND d.skipped = 1)
             AS skipped
         FROM newsletter_editions e
         WHERE e.tenant_id = ?1 AND e.campaign_id = ?2
         ORDER BY e.opened_at DESC, e.id DESC
         LIMIT ?3",
        (tenant_id, campaign_id, i64::from(limit)),
    )
    .await
}

/// Where an edition stands in one time zone.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ZoneProgress {
    /// The subscribers' IANA zone, the shop's for those without one.
    pub zone: String,
    /// Mailed (or tried) so far.
    pub delivered: i64,
    /// Confirmed (in the edition's language) and not mailed yet.
    pub owed: i64,
}

/// An edition's progress by the subscribers' time zone; `shop_zone` stands
/// in for subscribers without one. Only zones with someone mailed or owed.
pub async fn edition_progress(
    db: &Db,
    tenant_id: &str,
    edition_id: &str,
    language: Option<&str>,
    shop_zone: &str,
) -> anyhow::Result<Vec<ZoneProgress>> {
    db.fetch_all(
        "SELECT zone, SUM(mailed) AS delivered, SUM(waiting) AS owed FROM (
            SELECT COALESCE(s.time_zone, ?4) AS zone,
                CASE WHEN d.subscriber_id IS NOT NULL AND d.skipped = 0 THEN 1 ELSE 0 END
                    AS mailed,
                CASE WHEN d.subscriber_id IS NULL AND s.status = 'confirmed'
                     AND (?3 IS NULL OR s.language = ?3) THEN 1 ELSE 0 END AS waiting
            FROM newsletter_subscribers s
            LEFT JOIN newsletter_deliveries d INDEXED BY sqlite_autoindex_newsletter_deliveries_1
              ON d.tenant_id = s.tenant_id AND d.edition_id = ?2 AND d.subscriber_id = s.id
            WHERE s.tenant_id = ?1)
         GROUP BY zone
         HAVING SUM(mailed) + SUM(waiting) > 0
         ORDER BY zone",
        (tenant_id, edition_id, language, shop_zone),
    )
    .await
}

/// Up to `limit` confirmed subscribers in `zone` (in `language`, when
/// given) that the edition was not mailed to yet, by id; `shop_zone` stands
/// in for subscribers without one.
pub async fn undelivered_in_zone(
    db: &Db,
    tenant_id: &str,
    edition_id: &str,
    language: Option<&str>,
    shop_zone: &str,
    zone: &str,
    limit: u32,
) -> anyhow::Result<Vec<SubscriberRow>> {
    db.fetch_all(
        "SELECT s.* FROM newsletter_subscribers s
         WHERE s.tenant_id = ?1 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND COALESCE(s.time_zone, ?4) = ?5
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d
                 INDEXED BY sqlite_autoindex_newsletter_deliveries_1
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?6",
        (
            tenant_id,
            edition_id,
            language,
            shop_zone,
            zone,
            i64::from(limit),
        ),
    )
    .await
}
