//! Queries over `invoicing_documents`, for the invoicing pages of a tenant.

use timada_core::db::{Db, FromRow};

use crate::value_object::format_amount_fr;

/// One row of `invoicing_documents`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DocumentRow {
    pub tenant_id: String,
    /// The index stream.
    pub id: String,
    /// `invoice` or `credit_note`.
    pub kind: String,
    pub series: String,
    pub number: i64,
    /// `F2026-000042`.
    pub label: String,
    pub order_id: String,
    pub order_reference: String,
    pub buyer_name: String,
    pub total_cents: i64,
    pub currency: String,
    /// Unix seconds.
    pub issued_at: i64,
    /// [`Regime`](crate::tax::Regime) as text.
    pub regime: String,
    pub tax_country: String,
    pub eu_distance_sales_cents: i64,
    pub eu_threshold_passed: bool,
    pub company_id: String,
}

impl DocumentRow {
    pub fn is_credit_note(&self) -> bool {
        self.kind == crate::DocumentKind::CreditNote.as_str()
    }

    /// The total as a French reader reads it (`24,00 €`).
    pub fn total_label(&self) -> String {
        format_amount_fr(self.total_cents as u64, &self.currency)
    }
}

/// The documents of a tenant, in the order of their series and numbers,
/// newest first.
pub async fn documents_of_tenant(
    db: &Db,
    tenant_id: &str,
    offset: i64,
    limit: i64,
) -> anyhow::Result<Vec<DocumentRow>> {
    db.fetch_all(
        "SELECT * FROM invoicing_documents WHERE tenant_id = ?1
         ORDER BY issued_at DESC, series DESC, number DESC LIMIT ?2 OFFSET ?3",
        (tenant_id, limit, offset),
    )
    .await
}

pub async fn count_documents(db: &Db, tenant_id: &str) -> anyhow::Result<i64> {
    db.fetch_scalar(
        "SELECT COUNT(*) FROM invoicing_documents WHERE tenant_id = ?1",
        (tenant_id,),
    )
    .await
}

/// The invoice and the credit notes of an order, in the order they were
/// issued.
pub async fn documents_of_order(
    db: &Db,
    tenant_id: &str,
    order_id: &str,
) -> anyhow::Result<Vec<DocumentRow>> {
    db.fetch_all(
        "SELECT * FROM invoicing_documents WHERE tenant_id = ?1 AND order_id = ?2
         ORDER BY issued_at, kind DESC, number",
        (tenant_id, order_id),
    )
    .await
}

/// One line of a One-Stop Shop return: the sales taxed in a country at a
/// rate, credit notes deducted.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct OssRow {
    pub country: String,
    pub rate_bps: i64,
    pub currency: String,
    pub base_cents: i64,
    pub vat_cents: i64,
}

/// A company's One-Stop Shop return of `[from, to)` (Unix seconds), every
/// tenant of it together: the VAT of other EU countries charged to their
/// consumers, by country and rate.
pub async fn oss_return(
    db: &Db,
    company_id: &str,
    from: i64,
    to: i64,
) -> anyhow::Result<Vec<OssRow>> {
    db.fetch_all(
        "SELECT country, rate_bps, currency,
            SUM(CASE kind WHEN 'credit_note' THEN -base_cents ELSE base_cents END) AS base_cents,
            SUM(CASE kind WHEN 'credit_note' THEN -vat_cents ELSE vat_cents END) AS vat_cents
         FROM invoicing_vat_lines
         WHERE company_id = ?1 AND regime = 'oss' AND issued_at >= ?2 AND issued_at < ?3
         GROUP BY country, rate_bps, currency
         ORDER BY country, rate_bps DESC",
        (company_id, from, to),
    )
    .await
}
