//! SQL list read model of credit notes: what the admin shows under an invoice
//! and next to each refund. Fed by the `invoice-credit-note-list` subscription.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::CreditNoteIssued;

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const CREDIT_NOTE_LIST_SUBSCRIPTION: &str = "invoice-credit-note-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CreditNoteListRow {
    pub credit_note_id: String,
    pub credit_note_number: String,
    pub refund_id: String,
    pub invoice_id: String,
    pub order_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub reason: String,
    pub issued_at: i64,
}

pub fn credit_note_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CREDIT_NOTE_LIST_SUBSCRIPTION)
        .handler(insert_on_credit_note_issued())
        .strict()
}

/// The credit notes of one invoice, oldest first.
pub async fn credit_notes_of_invoice(
    db: &SqlitePool,
    invoice_id: &str,
) -> sqlx::Result<Vec<CreditNoteListRow>> {
    sqlx::query_as(
        "SELECT credit_note_id, credit_note_number, refund_id, invoice_id, order_id,
                amount_minor, currency, reason, issued_at
         FROM invoice_credit_note_list
         WHERE invoice_id = ?
         ORDER BY issued_at, credit_note_number",
    )
    .bind(invoice_id)
    .fetch_all(db)
    .await
}

/// The credit notes documenting the given refunds, in no particular order.
pub async fn credit_notes_of_refunds(
    db: &SqlitePool,
    refund_ids: &[String],
) -> sqlx::Result<Vec<CreditNoteListRow>> {
    if refund_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT credit_note_id, credit_note_number, refund_id, invoice_id, order_id,
                amount_minor, currency, reason, issued_at
         FROM invoice_credit_note_list
         WHERE refund_id IN (",
    );
    let mut bound = query.separated(", ");
    for id in refund_ids {
        bound.push_bind(id);
    }
    query.push(")");
    query.build_query_as().fetch_all(db).await
}

#[evento::subscription]
async fn insert_on_credit_note_issued<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CreditNoteIssued>,
) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    sqlx::query(
        "INSERT OR IGNORE INTO invoice_credit_note_list
            (credit_note_id, credit_note_number, refund_id, invoice_id, order_id,
             amount_minor, currency, reason, issued_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.credit_note_number)
    .bind(&event.data.refund_id)
    .bind(&event.data.invoice_id)
    .bind(&event.data.order_id)
    .bind(event.data.amount.minor)
    .bind(&event.data.amount.currency)
    .bind(&event.data.reason)
    .bind(event.timestamp as i64)
    .execute(&db)
    .await?;
    Ok(())
}
