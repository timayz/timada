//! Write-side commands for [`Invoice`](crate::aggregate::Invoice).
//!
//! Both are idempotent and both are dispatched only by the `invoice-issuance`
//! subscription — an invoice is a consequence of an order being paid, never
//! something an operator decides to create.

use evento::{AggregateExt as _, ProjectionAggregate as _};
use sqlx::SqlitePool;
use timada_core::Executor;
use timada_order::OrderView;

use crate::aggregate::{CreditNoteIssued, InvoiceIssued, InvoiceLine, Party};
use crate::numbering::next_number;
use crate::state::InvoiceConfig;
use crate::view::load_invoice;

/// Counter kinds — the identity of the two sequences, independent of how they
/// are spelled (that is the configured prefix's job).
const INVOICE_KIND: &str = "invoice";
const CREDIT_NOTE_KIND: &str = "credit_note";

/// The id of the invoice for `order_id`.
///
/// Derived rather than generated so the issuance subscription can replay
/// without keeping a mapping table, and so "does this order already have an
/// invoice?" is one lookup rather than a scan.
pub fn invoice_id(order_id: &str) -> String {
    evento::hash_ids(vec![order_id, "invoice"])
}

/// Bill a paid order.
///
/// Returns the invoice aggregate id. Re-issuing is a no-op — and the check
/// runs *before* a number is allocated, so a replayed event cannot burn one.
///
/// Everything is copied off the order as it stands right now: the buyer from
/// the shipping address and the order's email, the lines from the order's
/// lines, the totals from the order's own snapshotted tax split. Nothing is
/// recomputed, because the invoice has to agree with what was charged down to
/// the cent, not with what today's rates would make of it.
#[tracing::instrument(skip(executor, write_pool, config, order), fields(order_id = %order.id))]
pub async fn issue_invoice(
    executor: &Executor,
    write_pool: &SqlitePool,
    config: &InvoiceConfig,
    order: &OrderView,
) -> anyhow::Result<String> {
    let id = invoice_id(&order.id);

    if executor.has_event::<InvoiceIssued>(&id).await? {
        tracing::debug!(invoice_id = %id, "order already invoiced, skipping");
        return Ok(id);
    }

    let invoice_number = next_number(write_pool, INVOICE_KIND, &config.invoice_prefix).await?;

    let lines = order
        .lines
        .iter()
        .map(|line| InvoiceLine {
            description: line.title.clone(),
            quantity: line.quantity,
            unit_price_gross: line.unit_price,
            tax_rate_bps: line.tax_rate_bps,
            net: line.net,
            tax: line.tax,
            gross: line.line_total(),
        })
        .collect();

    let buyer = Party {
        name: order.shipping_address.full_name.clone(),
        street: order.shipping_address.street.clone(),
        city: order.shipping_address.city.clone(),
        postal_code: order.shipping_address.postal_code.clone(),
        country: order.shipping_address.country.clone(),
        email: order.email.clone(),
    };

    evento::append(&id)
        .original_version(0)
        .event(&InvoiceIssued {
            order_id: order.id.clone(),
            invoice_number: invoice_number.clone(),
            seller: config.seller.clone(),
            buyer,
            lines,
            total_net: order.total_net,
            total_tax: order.total_tax,
            total_gross: order.total,
        })
        .commit(executor)
        .await?;

    tracing::info!(invoice_id = %id, %invoice_number, "invoice issued");
    Ok(id)
}

/// Reverse an order's invoice in full.
///
/// An order that was never invoiced — cancelled before the charge landed, the
/// common case — has nothing to reverse, so this is a no-op rather than an
/// error. That is what lets the subscription call it on *every* cancellation
/// without first asking whether the order was ever paid.
#[tracing::instrument(skip(executor, write_pool, config))]
pub async fn issue_credit_note(
    executor: &Executor,
    write_pool: &SqlitePool,
    config: &InvoiceConfig,
    order_id: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let id = invoice_id(order_id);

    let Some(invoice) = load_invoice(executor, &id).await? else {
        tracing::debug!(invoice_id = %id, "order was never invoiced, nothing to credit");
        return Ok(());
    };

    // The replay that produced this view is the same read `has_event` would
    // do, so the loaded flag is the idempotency check.
    if let Some(number) = &invoice.credit_note_number {
        tracing::debug!(invoice_id = %id, credit_note_number = %number, "invoice already credited");
        return Ok(());
    }

    let credit_note_number =
        next_number(write_pool, CREDIT_NOTE_KIND, &config.credit_note_prefix).await?;

    invoice
        .write()?
        .event(&CreditNoteIssued {
            credit_note_number: credit_note_number.clone(),
            reason: reason.to_owned(),
        })
        .commit(executor)
        .await?;

    tracing::info!(invoice_id = %id, %credit_note_number, %reason, "credit note issued");
    Ok(())
}
