//! The subscription that keeps `invoicing_documents` current for one
//! tenant. A document never changes once issued, so its row is written from
//! the event alone; delivery may repeat (the subscription is at-least-once)
//! and rewrites the same row.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::{CreditNoteIssued, InvoiceIssued};
use crate::command::DocumentKind;
use crate::value_object::document_label;

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-invoicing/read-models";

/// The tenant whose stream a subscription follows, handed to its handlers.
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

/// Builds the subscription of one tenant over every routing key. `pool` is
/// the shared read-model database and must be allowed to write.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    tenant_id: &str,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(TenantId(tenant_id.to_owned()))
        .handler(on_invoice_issued())
        .handler(on_credit_note_issued())
        .any_routing_key()
}

#[evento::subscription]
async fn on_invoice_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<InvoiceIssued>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let data = &event.data;
    upsert(
        &pool,
        &tenant.0,
        Row {
            id: &data.document_id,
            kind: DocumentKind::Invoice,
            series: &data.series,
            number: data.number,
            order_id: &data.order_id,
            order_reference: &data.order_reference,
            buyer_name: &data.buyer.name,
            total_cents: data.total_cents,
            currency: &data.currency,
            issued_at: data.issued_at,
        },
    )
    .await
}

#[evento::subscription]
async fn on_credit_note_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<CreditNoteIssued>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let data = &event.data;
    upsert(
        &pool,
        &tenant.0,
        Row {
            id: &data.document_id,
            kind: DocumentKind::CreditNote,
            series: &data.series,
            number: data.number,
            order_id: &data.order_id,
            order_reference: &data.order_reference,
            buyer_name: &data.buyer.name,
            total_cents: data.total_cents,
            currency: &data.currency,
            issued_at: data.issued_at,
        },
    )
    .await
}

struct Row<'a> {
    id: &'a str,
    kind: DocumentKind,
    series: &'a str,
    number: u64,
    order_id: &'a str,
    order_reference: &'a str,
    buyer_name: &'a str,
    total_cents: u64,
    currency: &'a str,
    issued_at: u64,
}

async fn upsert(pool: &SqlitePool, tenant_id: &str, row: Row<'_>) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO invoicing_documents (tenant_id, id, kind, series, number, label, order_id,
            order_reference, buyer_name, total_cents, currency, issued_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (tenant_id, id) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(row.id)
    .bind(row.kind.as_str())
    .bind(row.series)
    .bind(row.number as i64)
    .bind(document_label(row.series, row.number))
    .bind(row.order_id)
    .bind(row.order_reference)
    .bind(row.buyer_name)
    .bind(row.total_cents as i64)
    .bind(row.currency)
    .bind(row.issued_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}
