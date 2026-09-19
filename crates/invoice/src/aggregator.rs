use timada_core::{Address, Money};

use crate::value_object::InvoiceLine;

#[evento::aggregate(name = "timada-invoice/Invoice")]
pub enum Invoice {
    /// The order was placed: an unnumbered invoice exists for it.
    InvoiceDrafted {
        order_id: String,
        customer_id: String,
        billing_address: Address,
        lines: Vec<InvoiceLine>,
        shipping_fee: Money,
        handling_fee: Money,
    },

    /// The order's promo code or voucher: `amount` comes off the total.
    /// Committed together with `InvoiceDrafted`, never on its own.
    InvoiceDiscountApplied { label: String, amount: Money },

    /// The order was paid: the invoice got its legal number.
    InvoiceIssued { invoice_number: String },

    /// The order will not be fulfilled.
    InvoiceVoided { reason: String },
}

/// A credit note ("avoir"): the legal counterpart of a refund. An issued
/// invoice is never edited — what goes back to the customer is credited
/// against it, under its own number.
#[evento::aggregate(name = "timada-invoice/CreditNote")]
pub enum CreditNote {
    CreditNoteIssued {
        credit_note_number: String,
        /// The refund this note documents (the payment context's event id).
        refund_id: String,
        invoice_id: String,
        invoice_number: String,
        order_id: String,
        amount: Money,
        reason: String,
    },
}
