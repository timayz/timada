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

    /// The order was paid: the invoice got its legal number.
    InvoiceIssued { invoice_number: String },

    /// The order will not be fulfilled.
    InvoiceVoided { reason: String },
}
