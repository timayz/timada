use timada_core::{Address, Money};

use timada_tax::{BusinessBuyer, ReverseChargeProof, TaxTreatment, VatLine};

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
    InvoiceDiscountApplied {
        label: String,
        amount: Money,
    },

    /// How the invoiced amounts are taxed: the zone, and the VAT per rate that
    /// an invoice must show. Committed together with `InvoiceDrafted`, never on
    /// its own; invoices of orders older than tax zones have none.
    InvoiceTaxed {
        zone_code: String,
        treatment: TaxTreatment,
        vat_lines: Vec<VatLine>,
    },

    /// The order was paid: the invoice got its legal number.
    /// The invoice is a business's: its name and VAT number, which the
    /// document must show. Committed together with `InvoiceDrafted`.
    InvoiceBuyerIdentified {
        buyer: BusinessBuyer,
    },

    /// The sale is an intra-community supply, exempt on this proof; the
    /// invoice is taxed like an export (`InvoiceTaxed` says `Export`) and its
    /// document says "autoliquidation". Committed together with
    /// `InvoiceDrafted` and `InvoiceBuyerIdentified`.
    InvoiceReverseCharged {
        proof: ReverseChargeProof,
    },

    InvoiceIssued {
        invoice_number: String,
    },

    /// The order will not be fulfilled.
    InvoiceVoided {
        reason: String,
    },
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
