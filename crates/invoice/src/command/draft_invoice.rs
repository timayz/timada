use evento::Executor;
use timada_core::{Address, Money};

use crate::{
    aggregator::{
        InvoiceBuyerIdentified, InvoiceDiscountApplied, InvoiceDrafted, InvoiceReverseCharged,
        InvoiceTaxed,
    },
    error::InvoiceError,
    value_object::{InvoiceDiscount, InvoiceLine, InvoiceTax, invoice_total},
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
    pub discount: Option<InvoiceDiscount>,
    /// The order's VAT summary, as its `OrderTaxed` recorded it.
    pub tax: Option<InvoiceTax>,
    /// The business the invoice is for, and the proof of its reverse charge
    /// when the sale is exempt; `None` for a consumer's invoice.
    pub business: Option<timada_tax::BusinessPurchase>,
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
        let (_, total) = invoice_total(&cmd.lines, &cmd.shipping_fee, &cmd.handling_fee)?;
        if let Some(discount) = &cmd.discount {
            discount.amount.same_currency(&total)?;
            if !discount.amount.is_positive() || discount.amount.minor > total.minor {
                return Err(InvoiceError::InvalidDiscount);
            }
        }

        let id = invoice_id(&cmd.order_id);
        let mut write = evento::append(&id);
        write.routing_key_opt(routing_key).event(&InvoiceDrafted {
            order_id: cmd.order_id.clone(),
            customer_id: cmd.customer_id,
            billing_address: cmd.billing_address,
            lines: cmd.lines,
            shipping_fee: cmd.shipping_fee,
            handling_fee: cmd.handling_fee,
        });
        if let Some(tax) = cmd.tax {
            write.event(&InvoiceTaxed {
                zone_code: tax.zone_code,
                treatment: tax.treatment,
                vat_lines: tax.vat_lines,
            });
        }
        if let Some(business) = cmd.business {
            write.event(&InvoiceBuyerIdentified {
                buyer: business.buyer,
            });
            if let Some(proof) = business.reverse_charge {
                write.event(&InvoiceReverseCharged { proof });
            }
        }
        if let Some(discount) = cmd.discount {
            write.event(&InvoiceDiscountApplied {
                label: discount.label,
                amount: discount.amount,
            });
        }
        let result = write.commit(self.executor).await;

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
