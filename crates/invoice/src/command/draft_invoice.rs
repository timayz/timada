use evento::Executor;
use timada_core::{Address, Money};

use crate::{
    aggregator::InvoiceDrafted,
    error::InvoiceError,
    value_object::{InvoiceLine, invoice_total},
};

use super::invoice_id;

#[derive(Debug, Clone)]
pub struct DraftInvoice {
    pub order_id: String,
    pub customer_id: String,
    pub billing_address: Address,
    pub lines: Vec<InvoiceLine>,
    pub shipping_fee: Money,
    pub handling_fee: Money,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Drafts the invoice of an order. The id is derived from the order id,
    /// so a redelivery of `OrderPlaced` finds the draft and returns its id.
    pub async fn draft_invoice(
        &self,
        cmd: DraftInvoice,
        routing_key: Option<String>,
    ) -> Result<String, InvoiceError> {
        if cmd.order_id.trim().is_empty() {
            return Err(InvoiceError::Required("order_id"));
        }
        if cmd.lines.is_empty() {
            return Err(InvoiceError::NoLines);
        }
        // Rejects mixed currencies and overflow before anything is written.
        invoice_total(&cmd.lines, &cmd.shipping_fee, &cmd.handling_fee)?;

        let id = invoice_id(&cmd.order_id);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&InvoiceDrafted {
                order_id: cmd.order_id.clone(),
                customer_id: cmd.customer_id,
                billing_address: cmd.billing_address,
                lines: cmd.lines,
                shipping_fee: cmd.shipping_fee,
                handling_fee: cmd.handling_fee,
            })
            .commit(self.executor)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(invoice_id = %id, order_id = %cmd.order_id, "invoice drafted");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => Ok(id),
            Err(err) => Err(err.into()),
        }
    }
}
