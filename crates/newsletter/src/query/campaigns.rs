//! Queries over `newsletter_campaigns`, `newsletter_editions`,
//! `newsletter_edition_zones`, `newsletter_audience` and
//! `newsletter_deliveries`: the campaigns page of a tenant, and what the
//! sending still owes.
//!
//! The counts are the read model's own columns and rows, kept as the
//! deliveries and the subscribers change (see `read_model`): a page never
//! counts over the deliveries, nor the sending over the subscribers.
//!
//! What is owed is walked by subscriber id, from where the last walk
//! stopped, and only the part of a node's [`Share`]: each node mails its
//! own subscribers, and goes on from the last one it looked at rather than
//! through everyone mailed already.
//!
//! Every lookup in `newsletter_deliveries` names its index (`INDEXED BY`):
//! Turso's planner costs each column of the composite primary key as unique
//! on its own and so takes whichever index pins `tenant_id`, then walks
//! every delivery of the tenant once per outer row
//! (tursodatabase/turso#8664). The primary key's index is the one SQLite
//! names for a table's `PRIMARY KEY` constraint. Issue #336 has the plans
//! and timings; `tests/query_plans.rs` runs them again.

use std::collections::BTreeMap;

use timada_core::Share;
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

/// The tenant's campaigns, newest first.
pub async fn campaigns_of_tenant(
    db: &Db,
    tenant_id: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<CampaignRow>> {
    db.fetch_all(
        "SELECT * FROM newsletter_campaigns
         WHERE tenant_id = ?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2 OFFSET ?3",
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
        "SELECT * FROM newsletter_campaigns WHERE tenant_id = ?1 AND id = ?2",
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
/// campaign was not mailed to yet, `share`'s part of them, by id from the
/// first after `after` (`""` for the first of all). `campaign_id` is what
/// the deliveries are keyed by: the campaign sent at once, or the edition of
/// a scheduled one. The read model is a moment behind: one claimed a moment
/// ago may come back, and its claim is then refused.
pub async fn undelivered_subscribers(
    db: &Db,
    tenant_id: &str,
    campaign_id: &str,
    language: Option<&str>,
    share: Share,
    after: &str,
    limit: u32,
) -> anyhow::Result<Vec<SubscriberRow>> {
    db.fetch_all(
        "SELECT s.* FROM newsletter_subscribers s
            INDEXED BY sqlite_autoindex_newsletter_subscribers_1
         WHERE s.tenant_id = ?1 AND s.id > ?4 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND s.shard % ?5 = ?6
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d
                 INDEXED BY sqlite_autoindex_newsletter_deliveries_1
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?7",
        (
            tenant_id,
            campaign_id,
            language,
            after,
            share.of as i64,
            share.index as i64,
            i64::from(limit),
        ),
    )
    .await
}

/// Whether any confirmed subscriber (in `language`, when given) is still
/// owed the campaign (or the edition `campaign_id` names), whichever node's
/// part they are.
pub async fn anyone_owed(
    db: &Db,
    tenant_id: &str,
    campaign_id: &str,
    language: Option<&str>,
) -> anyhow::Result<bool> {
    Ok(
        !undelivered_subscribers(db, tenant_id, campaign_id, language, Share::ALONE, "", 1)
            .await?
            .is_empty(),
    )
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
            "SELECT COALESCE(SUM(confirmed), 0) FROM newsletter_audience
             WHERE tenant_id = ?1 AND (?2 IS NULL OR language = ?2)",
            (tenant_id, language),
        )
        .await?;
    Ok(count.max(0) as u64)
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
        "SELECT * FROM newsletter_editions
         WHERE tenant_id = ?1 AND campaign_id = ?2
         ORDER BY opened_at DESC, id DESC
         LIMIT ?3",
        (tenant_id, campaign_id, i64::from(limit)),
    )
    .await
}

/// Where an edition stands in one time zone, from the read model's counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZoneProgress {
    /// The subscribers' IANA zone, the shop's for those without one.
    pub zone: String,
    /// Mailed (or tried) so far, counted in the zone the subscriber had
    /// when mailed.
    pub delivered: i64,
    /// Confirmed (in the edition's language) now, mailed already or not.
    pub audience: i64,
}

/// An edition's progress by the subscribers' time zone; `shop_zone` stands
/// in for subscribers without one. Only zones with someone mailed or on the
/// list, by zone.
pub async fn edition_progress(
    db: &Db,
    tenant_id: &str,
    edition_id: &str,
    language: Option<&str>,
    shop_zone: &str,
) -> anyhow::Result<Vec<ZoneProgress>> {
    let conn = db.connect()?;
    let mailed: Vec<(String, i64)> = timada_core::db::fetch_all(
        &conn,
        "SELECT time_zone, mailed FROM newsletter_edition_zones
         WHERE tenant_id = ?1 AND edition_id = ?2",
        (tenant_id, edition_id),
    )
    .await?;
    let audience: Vec<(String, i64)> = timada_core::db::fetch_all(
        &conn,
        "SELECT time_zone, confirmed FROM newsletter_audience
         WHERE tenant_id = ?1 AND (?2 IS NULL OR language = ?2)",
        (tenant_id, language),
    )
    .await?;
    let mut zones: BTreeMap<String, ZoneProgress> = BTreeMap::new();
    for (time_zone, delivered, confirmed) in mailed
        .into_iter()
        .map(|(time_zone, count)| (time_zone, count, 0))
        .chain(
            audience
                .into_iter()
                .map(|(time_zone, count)| (time_zone, 0, count)),
        )
    {
        let zone = if time_zone.is_empty() {
            shop_zone.to_owned()
        } else {
            time_zone
        };
        let progress = zones.entry(zone.clone()).or_insert_with(|| ZoneProgress {
            zone,
            ..ZoneProgress::default()
        });
        progress.delivered += delivered;
        progress.audience += confirmed;
    }
    Ok(zones
        .into_values()
        .filter(|zone| zone.delivered > 0 || zone.audience > 0)
        .collect())
}

/// Up to `limit` confirmed subscribers in `zone` (in `language`, when
/// given) that the edition was not mailed to yet, `share`'s part of them, by
/// id from the first after `after`; `shop_zone` stands in for subscribers
/// without one.
#[allow(clippy::too_many_arguments)]
pub async fn undelivered_in_zone(
    db: &Db,
    tenant_id: &str,
    edition_id: &str,
    language: Option<&str>,
    shop_zone: &str,
    zone: &str,
    share: Share,
    after: &str,
    limit: u32,
) -> anyhow::Result<Vec<SubscriberRow>> {
    db.fetch_all(
        "SELECT s.* FROM newsletter_subscribers s
            INDEXED BY sqlite_autoindex_newsletter_subscribers_1
         WHERE s.tenant_id = ?1 AND s.id > ?6 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND COALESCE(s.time_zone, ?4) = ?5
           AND s.shard % ?7 = ?8
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d
                 INDEXED BY sqlite_autoindex_newsletter_deliveries_1
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?9",
        (
            tenant_id,
            edition_id,
            language,
            shop_zone,
            zone,
            after,
            share.of as i64,
            share.index as i64,
            i64::from(limit),
        ),
    )
    .await
}
