//! Queries over `discounts_codes`, for the discount pages of a tenant.

use timada_core::db::{Db, FromRow};

use crate::value_object::DiscountValue;

/// One row of `discounts_codes`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DiscountRow {
    pub tenant_id: String,
    pub id: String,
    pub code: String,
    pub percent_bps: Option<i64>,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    /// Unix seconds.
    pub starts_at: Option<i64>,
    /// Unix seconds, excluded.
    pub ends_at: Option<i64>,
    pub max_uses: Option<i64>,
    pub once_per_email: bool,
    pub status: String,
    pub created_by: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

impl DiscountRow {
    /// What it takes off.
    pub fn value(&self) -> DiscountValue {
        match (self.percent_bps, self.amount_cents, &self.currency) {
            (None, Some(cents), Some(currency)) => DiscountValue::Amount {
                cents: cents as u64,
                currency: currency.clone(),
            },
            (bps, _, _) => DiscountValue::Percent(bps.unwrap_or(0) as u32),
        }
    }
}

/// The tenant's discounts, newest first; archived ones only when asked.
pub async fn discounts_of_tenant(
    db: &Db,
    tenant_id: &str,
    include_archived: bool,
) -> anyhow::Result<Vec<DiscountRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, code, percent_bps, amount_cents, currency, starts_at, ends_at,
            max_uses, once_per_email, status, created_by, created_at, updated_at
         FROM discounts_codes
         WHERE tenant_id = ?1 AND (?2 OR status <> 'archived')
         ORDER BY created_at DESC, id DESC",
        (tenant_id, include_archived),
    )
    .await
}
