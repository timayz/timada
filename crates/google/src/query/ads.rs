//! Queries over the Google Ads part of the read model: which shops' passes
//! are due, what the passes asked Google Ads on a day, and what an account
//! cost and brought over a period.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

/// A shop whose daily read of Google Ads is due, and the account to read.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DueAds {
    pub tenant_id: String,
    pub customer_id: String,
}

/// The statement of [`ads_due`]: the accounts by shop, each matched to its
/// watch by key.
pub(super) const ADS_DUE: &str = "SELECT a.tenant_id, a.customer_id
     FROM google_ads_accounts a
     LEFT JOIN google_ads_watch w ON w.tenant_id = a.tenant_id
     WHERE a.tenant_id > ?1 AND a.lapsed = 0
        AND (w.tenant_id IS NULL OR w.customer_id <> a.customer_id OR w.next_at <= ?2)
     ORDER BY a.tenant_id
     LIMIT ?3";

/// The shops whose pass is due at `now` (unix seconds), by id after
/// `after`, `limit` at most: connected, their grant honoured, an account
/// chosen, and never read, or not since the account changed, or due.
pub async fn ads_due(db: &Db, now: i64, after: &str, limit: i64) -> Result<Vec<DueAds>> {
    db.fetch_all(ADS_DUE, (after, now, limit)).await
}

/// The statement of [`ads_calls_on`].
pub(super) const CALLS_ON: &str =
    "SELECT COALESCE(SUM(calls), 0) FROM google_ads_calls WHERE day = ?1";

/// How many questions the shops' passes asked Google Ads on `day`
/// (`2026-11-20`, UTC), all shops together.
pub async fn ads_calls_on(db: &Db, day: &str) -> Result<i64> {
    db.fetch_scalar(CALLS_ON, (day,)).await
}

/// The last day of the account the shop holds numbers for; `None` before
/// the first.
pub async fn last_ads_day(db: &Db, tenant_id: &str, customer_id: &str) -> Result<Option<String>> {
    let day: Option<(String,)> = db
        .fetch_optional(
            "SELECT day FROM google_ads_days
             WHERE tenant_id = ?1 AND customer_id = ?2 ORDER BY day DESC LIMIT 1",
            (tenant_id, customer_id),
        )
        .await?;
    Ok(day.map(|(day,)| day))
}

/// What ads cost and brought over a period.
#[derive(Debug, Clone, Copy, Default, FromRow, PartialEq, Eq)]
pub struct AdsFigures {
    /// In millionths of the account's currency.
    pub cost_micros: i64,
    pub impressions: i64,
    pub clicks: i64,
    /// In thousandths.
    pub conversions_milli: i64,
    /// In millionths of the account's currency.
    pub value_micros: i64,
}

impl AdsFigures {
    /// What the clicks cost, in hundredths of the account's currency.
    pub fn cost_cents(&self) -> i64 {
        (self.cost_micros + 5_000) / 10_000
    }

    /// What the conversions were worth, in hundredths of the account's
    /// currency.
    pub fn value_cents(&self) -> i64 {
        (self.value_micros + 5_000) / 10_000
    }

    /// The return on ad spend as Google Ads counts it, in hundredths: 345
    /// is 3.45 of conversion value for 1 spent. `None` when nothing was
    /// spent.
    pub fn return_hundredths(&self) -> Option<i64> {
        (self.cost_micros > 0).then(|| {
            ((i128::from(self.value_micros) * 100 + i128::from(self.cost_micros) / 2)
                / i128::from(self.cost_micros)) as i64
        })
    }

    /// What a click cost on average, in hundredths of the account's
    /// currency; `None` without a click.
    pub fn click_cost_cents(&self) -> Option<i64> {
        (self.clicks > 0).then(|| (self.cost_micros / self.clicks + 5_000) / 10_000)
    }

    /// Clicks per impression, in hundredths of a percent; `None` without
    /// an impression.
    pub fn click_rate_bps(&self) -> Option<i64> {
        (self.impressions > 0)
            .then(|| (self.clicks * 10_000 + self.impressions / 2) / self.impressions)
    }
}

/// A period's totals, over how many days it holds numbers, and the
/// currency they are in.
#[derive(Debug, Clone, Default, FromRow, PartialEq, Eq)]
pub struct AdsTotals {
    pub days: i64,
    /// Empty for a period with no numbers.
    pub currency: String,
    pub cost_micros: i64,
    pub impressions: i64,
    pub clicks: i64,
    pub conversions_milli: i64,
    pub value_micros: i64,
}

impl AdsTotals {
    pub fn figures(&self) -> AdsFigures {
        AdsFigures {
            cost_micros: self.cost_micros,
            impressions: self.impressions,
            clicks: self.clicks,
            conversions_milli: self.conversions_milli,
            value_micros: self.value_micros,
        }
    }
}

/// The statement of [`ads_totals`].
pub(super) const TOTALS: &str = "SELECT COUNT(*) AS days,
        COALESCE(MAX(currency), '') AS currency,
        COALESCE(SUM(cost_micros), 0) AS cost_micros,
        COALESCE(SUM(impressions), 0) AS impressions,
        COALESCE(SUM(clicks), 0) AS clicks,
        COALESCE(SUM(conversions_milli), 0) AS conversions_milli,
        COALESCE(SUM(value_micros), 0) AS value_micros
     FROM google_ads_days
     WHERE tenant_id = ?1 AND customer_id = ?2 AND day >= ?3 AND day <= ?4";

/// What the account cost and brought from `first` to `last`
/// (`2026-11-20`), both included.
pub async fn ads_totals(
    db: &Db,
    tenant_id: &str,
    customer_id: &str,
    first: &str,
    last: &str,
) -> Result<AdsTotals> {
    db.fetch_one(TOTALS, (tenant_id, customer_id, first, last))
        .await
}

/// A campaign, a kind of device, an ad group, a product or a search over a
/// period.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct AdsLine {
    /// The campaign's or the ad group's id, the device, the feed's item id
    /// (in lower case) or the search.
    pub key: String,
    /// What Google Ads calls the campaign or the ad group; empty for the
    /// other kinds.
    pub name: String,
    pub cost_micros: i64,
    pub impressions: i64,
    pub clicks: i64,
    pub conversions_milli: i64,
    pub value_micros: i64,
}

impl AdsLine {
    pub fn figures(&self) -> AdsFigures {
        AdsFigures {
            cost_micros: self.cost_micros,
            impressions: self.impressions,
            clicks: self.clicks,
            conversions_milli: self.conversions_milli,
            value_micros: self.value_micros,
        }
    }
}

/// What an account's rows are added up by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum By {
    Campaign,
    Device,
    AdGroup,
    Product,
    Term,
}

/// The statement that adds a period's rows up by one thing: the period is
/// one range of the table's key. A campaign or an ad group renamed in the
/// period reads under one of its names. The products and the searches are
/// cut to `?5` rows.
pub(super) fn by_statement(by: By) -> String {
    let (table, key, name, limit) = match by {
        By::Campaign => ("google_ads_campaigns", "campaign_id", "MAX(name)", ""),
        By::Device => ("google_ads_campaigns", "device", "''", ""),
        By::AdGroup => ("google_ads_ad_groups", "ad_group_id", "MAX(name)", ""),
        By::Product => ("google_ads_products", "item_id", "''", " LIMIT ?5"),
        By::Term => ("google_ads_terms", "term", "''", " LIMIT ?5"),
    };
    format!(
        "SELECT {key} AS key, {name} AS name,
            SUM(cost_micros) AS cost_micros, SUM(impressions) AS impressions,
            SUM(clicks) AS clicks, SUM(conversions_milli) AS conversions_milli,
            SUM(value_micros) AS value_micros
         FROM {table}
         WHERE tenant_id = ?1 AND customer_id = ?2 AND day >= ?3 AND day <= ?4
         GROUP BY {key}
         ORDER BY cost_micros DESC, clicks DESC, {key}{limit}"
    )
}

/// The account's campaigns from `first` to `last`, the one that cost the
/// most first.
pub async fn ads_by_campaign(
    db: &Db,
    tenant_id: &str,
    customer_id: &str,
    first: &str,
    last: &str,
) -> Result<Vec<AdsLine>> {
    db.fetch_all(
        &by_statement(By::Campaign),
        (tenant_id, customer_id, first, last),
    )
    .await
}

/// The account's numbers from `first` to `last` by kind of device.
pub async fn ads_by_device(
    db: &Db,
    tenant_id: &str,
    customer_id: &str,
    first: &str,
    last: &str,
) -> Result<Vec<AdsLine>> {
    db.fetch_all(
        &by_statement(By::Device),
        (tenant_id, customer_id, first, last),
    )
    .await
}

/// The account's ad groups from `first` to `last`, the one that cost the
/// most first.
pub async fn ads_by_ad_group(
    db: &Db,
    tenant_id: &str,
    customer_id: &str,
    first: &str,
    last: &str,
) -> Result<Vec<AdsLine>> {
    db.fetch_all(
        &by_statement(By::AdGroup),
        (tenant_id, customer_id, first, last),
    )
    .await
}

/// The feed's items the account showed from `first` to `last`, the one
/// that cost the most first, `limit` at most.
pub async fn ads_by_product(
    db: &Db,
    tenant_id: &str,
    customer_id: &str,
    first: &str,
    last: &str,
    limit: i64,
) -> Result<Vec<AdsLine>> {
    db.fetch_all(
        &by_statement(By::Product),
        (tenant_id, customer_id, first, last, limit),
    )
    .await
}

/// The searches that brought the account a click from `first` to `last`,
/// the one that cost the most first, `limit` at most.
pub async fn ads_by_term(
    db: &Db,
    tenant_id: &str,
    customer_id: &str,
    first: &str,
    last: &str,
    limit: i64,
) -> Result<Vec<AdsLine>> {
    db.fetch_all(
        &by_statement(By::Term),
        (tenant_id, customer_id, first, last, limit),
    )
    .await
}
