//! Queries over the Merchant Center part of the read model: which shops'
//! passes are due, and where a shop's products stand.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

use crate::value_object::{MERCHANT_ACCEPTED, MERCHANT_PENDING, MERCHANT_REFUSED};

/// A shop whose daily read of Merchant Center is due, and the account to
/// read.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DueMerchant {
    pub tenant_id: String,
    pub account_id: String,
}

/// The statement of [`merchant_due`].
pub(super) const MERCHANT_DUE: &str = "SELECT a.tenant_id, a.account_id
     FROM google_merchant_accounts a
     LEFT JOIN google_merchant_watch w ON w.tenant_id = a.tenant_id
     WHERE a.tenant_id > ?1 AND a.lapsed = 0
        AND (w.tenant_id IS NULL OR w.account_id <> a.account_id OR w.next_at <= ?2)
     ORDER BY a.tenant_id
     LIMIT ?3";

/// The shops whose pass is due at `now` (unix seconds), by id after
/// `after`, `limit` at most.
pub async fn merchant_due(db: &Db, now: i64, after: &str, limit: i64) -> Result<Vec<DueMerchant>> {
    db.fetch_all(MERCHANT_DUE, (after, now, limit)).await
}

/// The Merchant Center account the shop's connection reads, as the read
/// model has it; `None` for a shop with none chosen, or disconnected.
pub async fn merchant_account_of(db: &Db, tenant_id: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = db
        .fetch_optional(
            "SELECT account_id FROM google_merchant_accounts WHERE tenant_id = ?1",
            (tenant_id,),
        )
        .await?;
    Ok(row.map(|(account_id,)| account_id))
}

/// Where a shop's products stand in Merchant Center, in one place (Shopping
/// ads, or the free listings).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MerchantTally {
    pub accepted: i64,
    pub pending: i64,
    pub refused: i64,
    /// Not sent there at all.
    pub absent: i64,
}

/// Where a shop's products stand in Merchant Center, as of its last read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MerchantStanding {
    /// Unix seconds of the last read that moved something.
    pub read_at: i64,
    pub ads: MerchantTally,
    pub listings: MerchantTally,
    /// The accepted products the read did not keep one by one: they count
    /// as accepted in both places.
    pub more: i64,
}

impl MerchantStanding {
    /// How many products Merchant Center holds.
    pub fn products(&self) -> i64 {
        let ads = self.ads;
        ads.accepted + ads.pending + ads.refused + ads.absent
    }

    /// Whether every product is accepted for Shopping ads: what ads wait
    /// for. `false` for a feed Merchant Center holds nothing of.
    pub fn all_accepted_for_ads(&self) -> bool {
        self.ads.accepted > 0
            && self.ads.pending == 0
            && self.ads.refused == 0
            && self.ads.absent == 0
    }
}

/// The statement of [`merchant_standing`]'s counts: every product of the
/// account, off its key.
pub(super) const TALLY: &str = "SELECT ads, listings, COUNT(*) AS products
     FROM google_merchant_products
     WHERE tenant_id = ?1 AND account_id = ?2
     GROUP BY ads, listings";

/// Where the shop's products stand in the account; `None` before the
/// first read.
pub async fn merchant_standing(
    db: &Db,
    tenant_id: &str,
    account_id: &str,
) -> Result<Option<MerchantStanding>> {
    let feed: Option<(i64, i64)> = db
        .fetch_optional(
            "SELECT read_at, more FROM google_merchant_feeds
             WHERE tenant_id = ?1 AND account_id = ?2",
            (tenant_id, account_id),
        )
        .await?;
    let Some((read_at, more)) = feed else {
        return Ok(None);
    };
    let mut standing = MerchantStanding {
        read_at,
        more,
        ..MerchantStanding::default()
    };
    let rows: Vec<(String, String, i64)> = db.fetch_all(TALLY, (tenant_id, account_id)).await?;
    let add = |tally: &mut MerchantTally, status: &str, products: i64| match status {
        MERCHANT_ACCEPTED => tally.accepted += products,
        MERCHANT_PENDING => tally.pending += products,
        MERCHANT_REFUSED => tally.refused += products,
        _ => tally.absent += products,
    };
    for (ads, listings, products) in rows {
        add(&mut standing.ads, &ads, products);
        add(&mut standing.listings, &listings, products);
    }
    standing.ads.accepted += more;
    standing.listings.accepted += more;
    Ok(Some(standing))
}

/// One product of the feed as Merchant Center has it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct MerchantProduct {
    pub item_id: String,
    /// 0 refused, 1 still looked at, 2 shown with something held against
    /// it, 3 fine.
    pub rank: i64,
    pub ads: String,
    pub listings: String,
}

/// The statement of [`merchant_problems`]: read in the order of the index
/// made for it.
pub(super) const PROBLEMS: &str = "SELECT item_id, rank, ads, listings
     FROM google_merchant_products INDEXED BY google_merchant_products_rank
     WHERE tenant_id = ?1 AND account_id = ?2 AND rank < 3
     ORDER BY rank, item_id
     LIMIT ?3";

/// The products Merchant Center does not simply accept, the refused ones
/// first, `limit` at most.
pub async fn merchant_problems(
    db: &Db,
    tenant_id: &str,
    account_id: &str,
    limit: i64,
) -> Result<Vec<MerchantProduct>> {
    db.fetch_all(PROBLEMS, (tenant_id, account_id, limit)).await
}

/// One item of the feed in Merchant Center; `None` for one it does not
/// hold, or one of the accepted ones the read did not keep.
pub async fn merchant_product(
    db: &Db,
    tenant_id: &str,
    account_id: &str,
    item_id: &str,
) -> Result<Option<MerchantProduct>> {
    db.fetch_optional(
        "SELECT item_id, rank, ads, listings FROM google_merchant_products
         WHERE tenant_id = ?1 AND account_id = ?2 AND item_id = ?3",
        (tenant_id, account_id, item_id),
    )
    .await
}

/// Something Merchant Center holds against a product.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct MerchantIssueRow {
    pub item_id: String,
    pub code: String,
    pub severity: String,
    /// The field of the feed it names; empty when it names none.
    pub attribute: String,
    /// `ads`, `listings`, or empty for both.
    pub context: String,
    pub description: String,
    pub detail: String,
    pub documentation: String,
}

/// The statement of [`merchant_issues`]: every issue of the account, off
/// its key, in the products' order.
pub(super) const ISSUES_OF: &str = "SELECT item_id, code, severity, attribute, context,
        description, detail, documentation
     FROM google_merchant_issues
     WHERE tenant_id = ?1 AND account_id = ?2
     ORDER BY item_id, n";

/// What Merchant Center holds against the products of the account, by
/// product.
pub async fn merchant_issues(
    db: &Db,
    tenant_id: &str,
    account_id: &str,
) -> Result<Vec<MerchantIssueRow>> {
    db.fetch_all(ISSUES_OF, (tenant_id, account_id)).await
}

/// What Merchant Center holds against one item.
pub async fn merchant_issues_of(
    db: &Db,
    tenant_id: &str,
    account_id: &str,
    item_id: &str,
) -> Result<Vec<MerchantIssueRow>> {
    db.fetch_all(
        "SELECT item_id, code, severity, attribute, context, description, detail, documentation
         FROM google_merchant_issues
         WHERE tenant_id = ?1 AND account_id = ?2 AND item_id = ?3
         ORDER BY n",
        (tenant_id, account_id, item_id),
    )
    .await
}

/// Something Merchant Center holds against the account itself.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct MerchantAccountIssueRow {
    pub title: String,
    /// `critical` (nothing is shown), `error` or `suggestion`.
    pub severity: String,
    pub detail: String,
    pub documentation: String,
}

/// The statement of [`merchant_account_issues`].
pub(super) const ACCOUNT_ISSUES: &str = "SELECT title, severity, detail, documentation
     FROM google_merchant_account_issues
     WHERE tenant_id = ?1 AND account_id = ?2
     ORDER BY n";

/// What Merchant Center holds against the account.
pub async fn merchant_account_issues(
    db: &Db,
    tenant_id: &str,
    account_id: &str,
) -> Result<Vec<MerchantAccountIssueRow>> {
    db.fetch_all(ACCOUNT_ISSUES, (tenant_id, account_id)).await
}
