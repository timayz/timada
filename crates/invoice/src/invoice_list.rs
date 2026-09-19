//! SQL list read model behind the admin's invoices section: one row per
//! invoice with its number, status and total. Fed by the `invoice-list`
//! subscription; the full invoice is served by [`crate::InvoiceView`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{InvoiceDiscountApplied, InvoiceDrafted, InvoiceIssued, InvoiceVoided},
    query::load_invoice,
    value_object::InvoiceStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const INVOICE_LIST_SUBSCRIPTION: &str = "invoice-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct InvoiceListRow {
    pub invoice_id: String,
    pub order_id: String,
    pub customer_id: String,
    /// `None` until the invoice is issued.
    pub invoice_number: Option<String>,
    /// [`InvoiceStatus::as_str`].
    pub status: String,
    pub total_minor: i64,
    pub currency: String,
    pub drafted_at: i64,
    pub issued_at: Option<i64>,
}

/// Filters for [`list_invoices`]; `number` matches the start of the legal number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListInvoices {
    pub status: Option<InvoiceStatus>,
    pub number: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListInvoices {
    fn default() -> Self {
        Self {
            status: None,
            number: None,
            limit: 50,
            offset: 0,
        }
    }
}

pub fn invoice_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(INVOICE_LIST_SUBSCRIPTION)
        .handler(refresh_on_invoice_drafted())
        .handler(refresh_on_invoice_issued())
        .handler(refresh_on_invoice_voided())
        // Committed together with `InvoiceDrafted`, which already writes the
        // total net of it.
        .skip::<InvoiceDiscountApplied>()
        .strict()
}

fn number_prefix(filter: &ListInvoices) -> Option<String> {
    let number = filter.number.as_deref()?.trim();
    (!number.is_empty()).then(|| format!("{}%", number.replace(['%', '_'], "")))
}

/// Invoices matching the filter, newest first.
pub async fn list_invoices(
    db: &SqlitePool,
    filter: &ListInvoices,
) -> sqlx::Result<Vec<InvoiceListRow>> {
    sqlx::query_as(
        "SELECT invoice_id, order_id, customer_id, invoice_number, status, total_minor,
                currency, drafted_at, issued_at
         FROM invoice_list
         WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR invoice_number LIKE ?2)
         ORDER BY drafted_at DESC, invoice_id DESC
         LIMIT ?3 OFFSET ?4",
    )
    .bind(filter.status.map(InvoiceStatus::as_str))
    .bind(number_prefix(filter))
    .bind(filter.limit)
    .bind(filter.offset)
    .fetch_all(db)
    .await
}

pub async fn count_invoices(db: &SqlitePool, filter: &ListInvoices) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM invoice_list
         WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR invoice_number LIKE ?2)",
    )
    .bind(filter.status.map(InvoiceStatus::as_str))
    .bind(number_prefix(filter))
    .fetch_one(db)
    .await
}

/// Writes the invoice as it stands now. Absolute values from the invoice
/// projection rather than per-event changes, so a redelivery changes nothing;
/// the two dates are only ever set once.
async fn refresh<E: Executor>(
    ctx: &Context<'_, E>,
    invoice_id: &str,
    drafted_at: Option<u64>,
    issued_at: Option<u64>,
) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(invoice) = load_invoice(ctx.executor, invoice_id).await? else {
        anyhow::bail!("invoice {invoice_id} cannot be loaded");
    };
    sqlx::query(
        "INSERT INTO invoice_list
            (invoice_id, order_id, customer_id, invoice_number, status, total_minor,
             currency, drafted_at, issued_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, COALESCE(?, 0), ?)
         ON CONFLICT (invoice_id) DO UPDATE SET
            invoice_number = excluded.invoice_number,
            status = excluded.status,
            total_minor = excluded.total_minor,
            issued_at = COALESCE(invoice_list.issued_at, excluded.issued_at)",
    )
    .bind(&invoice.id)
    .bind(&invoice.order_id)
    .bind(&invoice.customer_id)
    .bind(&invoice.invoice_number)
    .bind(invoice.status.as_str())
    .bind(invoice.total.minor)
    .bind(&invoice.total.currency)
    .bind(drafted_at.map(|at| at as i64))
    .bind(issued_at.map(|at| at as i64))
    .execute(&db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_invoice_drafted<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<InvoiceDrafted>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, Some(event.timestamp), None).await
}

#[evento::subscription]
async fn refresh_on_invoice_issued<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<InvoiceIssued>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, None, Some(event.timestamp)).await
}

#[evento::subscription]
async fn refresh_on_invoice_voided<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<InvoiceVoided>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, None, None).await
}
