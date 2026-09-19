//! The invoice as rendered for "Télécharger la facture" (PDF rendering is a
//! follow-up). Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::{Address, Money};

use crate::{
    aggregator::{
        Invoice, InvoiceDiscountApplied, InvoiceDrafted, InvoiceIssued, InvoiceTaxed, InvoiceVoided,
    },
    value_object::{InvoiceDiscount, InvoiceLine, InvoiceStatus, InvoiceTax, invoice_total},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct InvoiceView {
    pub id: String,
    pub order_id: String,
    pub customer_id: String,
    pub invoice_number: Option<String>,
    pub billing_address: Address,
    pub lines: Vec<InvoiceLine>,
    pub subtotal: Money,
    pub shipping_fee: Money,
    pub handling_fee: Money,
    /// The order's code and what it takes off; `total` is already net of it.
    pub discount: Option<InvoiceDiscount>,
    pub total: Money,
    /// The VAT per rate; `None` for invoices older than tax zones.
    pub tax: Option<InvoiceTax>,
    pub status: InvoiceStatus,
    pub voided_reason: Option<String>,
    /// Unix seconds of `InvoiceDrafted`.
    pub drafted_at: u64,
    /// Unix seconds of `InvoiceIssued`: the invoice's legal date.
    pub issued_at: Option<u64>,
}

pub fn create_projection<E: Executor>() -> Projection<E, InvoiceView> {
    Projection::new::<Invoice>()
        .handler(on_invoice_drafted())
        .handler(on_invoice_discount_applied())
        .handler(on_invoice_taxed())
        .handler(on_invoice_issued())
        .handler(on_invoice_voided())
        // The view gained `tax`, then its dates: snapshots taken with a
        // previous shape must not be decoded.
        .revision(2)
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<InvoiceView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_invoice_drafted(
    event: Event<InvoiceDrafted>,
    row: &mut InvoiceView,
) -> anyhow::Result<()> {
    let (subtotal, total) = invoice_total(
        &event.data.lines,
        &event.data.shipping_fee,
        &event.data.handling_fee,
    )?;
    row.id = event.aggregate_id.to_owned();
    row.drafted_at = event.timestamp;
    row.order_id = event.data.order_id;
    row.customer_id = event.data.customer_id;
    row.billing_address = event.data.billing_address;
    row.lines = event.data.lines;
    row.subtotal = subtotal;
    row.shipping_fee = event.data.shipping_fee;
    row.handling_fee = event.data.handling_fee;
    row.total = total;
    row.status = InvoiceStatus::Draft;
    Ok(())
}

#[evento::handler]
async fn on_invoice_discount_applied(
    event: Event<InvoiceDiscountApplied>,
    row: &mut InvoiceView,
) -> anyhow::Result<()> {
    row.total = row.total.checked_sub(&event.data.amount)?;
    row.discount = Some(InvoiceDiscount {
        label: event.data.label,
        amount: event.data.amount,
    });
    Ok(())
}

#[evento::handler]
async fn on_invoice_taxed(event: Event<InvoiceTaxed>, row: &mut InvoiceView) -> anyhow::Result<()> {
    row.tax = Some(InvoiceTax {
        zone_code: event.data.zone_code,
        treatment: event.data.treatment,
        vat_lines: event.data.vat_lines,
    });
    Ok(())
}

#[evento::handler]
async fn on_invoice_issued(
    event: Event<InvoiceIssued>,
    row: &mut InvoiceView,
) -> anyhow::Result<()> {
    row.issued_at = Some(event.timestamp);
    row.invoice_number = Some(event.data.invoice_number);
    row.status = InvoiceStatus::Issued;
    Ok(())
}

#[evento::handler]
async fn on_invoice_voided(
    event: Event<InvoiceVoided>,
    row: &mut InvoiceView,
) -> anyhow::Result<()> {
    row.voided_reason = Some(event.data.reason);
    row.status = InvoiceStatus::Voided;
    Ok(())
}
