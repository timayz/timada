//! The invoice as rendered for "Télécharger la facture" (PDF rendering is a
//! follow-up). Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::{Address, Money};

use crate::{
    aggregator::{Invoice, InvoiceDrafted, InvoiceIssued, InvoiceVoided},
    value_object::{InvoiceLine, InvoiceStatus, invoice_total},
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
    pub total: Money,
    pub status: InvoiceStatus,
    pub voided_reason: Option<String>,
}

pub fn create_projection<E: Executor>() -> Projection<E, InvoiceView> {
    Projection::new::<Invoice>()
        .handler(on_invoice_drafted())
        .handler(on_invoice_issued())
        .handler(on_invoice_voided())
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
async fn on_invoice_issued(
    event: Event<InvoiceIssued>,
    row: &mut InvoiceView,
) -> anyhow::Result<()> {
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
