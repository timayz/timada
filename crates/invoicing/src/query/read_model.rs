//! The subscription that keeps `invoicing_documents` current for every
//! tenant. A document never changes once issued, so its row is written from
//! the event alone; delivery may repeat (the subscription is at-least-once)
//! and rewrites the same row. One subscription follows every tenant's
//! streams: each row carries the tenant of the event that made it.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, Value, execute, params_from_iter};

use crate::aggregator::{CreditNoteIssued, InvoiceIssued};
use crate::command::DocumentKind;
use crate::value_object::{VatSubtotal, document_label};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-invoicing/read-models";

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_invoice_issued())
        .handler(on_credit_note_issued())
        .any_routing_key()
}

#[evento::subscription]
async fn on_invoice_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<InvoiceIssued>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let data = &event.data;
    upsert(
        &db,
        &event.tenant,
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
            company_id: &data.company_id,
            regime: &data.regime,
            tax_country: &data.tax_country,
            vat: &data.vat,
            eu_distance_sales_cents: data.eu_distance_sales_cents,
            eu_threshold_passed: data.eu_threshold_passed,
        },
    )
    .await
}

#[evento::subscription]
async fn on_credit_note_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<CreditNoteIssued>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let data = &event.data;
    upsert(
        &db,
        &event.tenant,
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
            company_id: &data.company_id,
            regime: &data.regime,
            tax_country: &data.tax_country,
            vat: &data.vat,
            eu_distance_sales_cents: 0,
            eu_threshold_passed: false,
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
    company_id: &'a str,
    regime: &'a str,
    tax_country: &'a str,
    vat: &'a [VatSubtotal],
    eu_distance_sales_cents: u64,
    eu_threshold_passed: bool,
}

/// Writes the row of a document and one row per VAT rate of it (what the
/// One-Stop Shop return sums), in one transaction.
async fn upsert(db: &Db, tenant_id: &str, row: Row<'_>) -> anyhow::Result<()> {
    let env = (tenant_id, row);
    db.write(&env, |tx, (tenant_id, row)| {
        Box::pin(async move {
            for subtotal in row.vat {
                execute(
                    tx,
                    "INSERT INTO invoicing_vat_lines (tenant_id, document_id, kind, regime, country,
                        rate_bps, base_cents, vat_cents, currency, issued_at, company_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT (tenant_id, document_id, rate_bps) DO NOTHING",
                    (
                        *tenant_id,
                        row.id,
                        row.kind.as_str(),
                        row.regime,
                        row.tax_country,
                        i64::from(subtotal.rate_bps),
                        subtotal.base_cents as i64,
                        subtotal.vat_cents as i64,
                        row.currency,
                        row.issued_at as i64,
                        row.company_id,
                    ),
                )
                .await?;
            }
            execute(
                tx,
                "INSERT INTO invoicing_documents (tenant_id, id, kind, series, number, label,
                    order_id, order_reference, buyer_name, total_cents, currency, issued_at,
                    regime, tax_country, eu_distance_sales_cents, eu_threshold_passed,
                    company_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                    ?17)
                 ON CONFLICT (tenant_id, id) DO NOTHING",
                // Seventeen values: more than a tuple of parameters holds.
                params_from_iter([
                    Value::from(*tenant_id),
                    Value::from(row.id),
                    Value::from(row.kind.as_str()),
                    Value::from(row.series),
                    Value::from(row.number as i64),
                    Value::from(document_label(row.series, row.number)),
                    Value::from(row.order_id),
                    Value::from(row.order_reference),
                    Value::from(row.buyer_name),
                    Value::from(row.total_cents as i64),
                    Value::from(row.currency),
                    Value::from(row.issued_at as i64),
                    Value::from(row.regime),
                    Value::from(row.tax_country),
                    Value::from(row.eu_distance_sales_cents as i64),
                    Value::from(row.eu_threshold_passed),
                    Value::from(row.company_id),
                ]),
            )
            .await?;
            Ok(())
        })
    })
    .await
}
