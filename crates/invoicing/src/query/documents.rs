//! Queries over `invoicing_documents`, for the invoicing pages of a tenant.

use sqlx::{FromRow, SqlitePool};

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
    pool: &SqlitePool,
    tenant_id: &str,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<DocumentRow>> {
    sqlx::query_as(
        "SELECT * FROM invoicing_documents WHERE tenant_id = ?1
         ORDER BY issued_at DESC, series DESC, number DESC LIMIT ?2 OFFSET ?3",
    )
    .bind(tenant_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn count_documents(pool: &SqlitePool, tenant_id: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM invoicing_documents WHERE tenant_id = ?1")
        .bind(tenant_id)
        .fetch_one(pool)
        .await
}

/// The invoice and the credit notes of an order, in the order they were
/// issued.
pub async fn documents_of_order(
    pool: &SqlitePool,
    tenant_id: &str,
    order_id: &str,
) -> sqlx::Result<Vec<DocumentRow>> {
    sqlx::query_as(
        "SELECT * FROM invoicing_documents WHERE tenant_id = ?1 AND order_id = ?2
         ORDER BY issued_at, kind DESC, number",
    )
    .bind(tenant_id)
    .bind(order_id)
    .fetch_all(pool)
    .await
}
