use bitcode::{Decode, Encode};
use timada_core::{Money, MoneyError};
use timada_tax::{TaxTreatment, VatLine};

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct InvoiceLine {
    pub product_id: String,
    pub label: String,
    pub quantity: u32,
    pub unit_price: Money,
}

impl InvoiceLine {
    pub fn total(&self) -> Result<Money, MoneyError> {
        self.unit_price.checked_mul(self.quantity)
    }
}

/// The reduction line of an invoice ("Code promo WELCOME10").
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct InvoiceDiscount {
    pub label: String,
    pub amount: Money,
}

/// The VAT summary of an invoice.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct InvoiceTax {
    pub zone_code: String,
    pub treatment: TaxTreatment,
    pub vat_lines: Vec<VatLine>,
}

impl InvoiceTax {
    /// The legal mention an invoice without VAT must carry.
    pub fn exemption_mention(&self) -> Option<&'static str> {
        self.treatment.exemption_mention()
    }

    /// The VAT regime to print on the invoice: the exemption above, or whose
    /// VAT a distance sale inside the EU carries.
    pub fn regime_mention(&self) -> Option<&'static str> {
        self.treatment.regime_mention()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum InvoiceStatus {
    #[default]
    Draft,
    Issued,
    Voided,
}

impl InvoiceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            InvoiceStatus::Draft => "draft",
            InvoiceStatus::Issued => "issued",
            InvoiceStatus::Voided => "voided",
        }
    }
}

/// Sums the lines and adds the fees. `Err` on a currency mismatch or overflow.
pub fn invoice_total(
    lines: &[InvoiceLine],
    shipping_fee: &Money,
    handling_fee: &Money,
) -> Result<(Money, Money), MoneyError> {
    let currency = lines
        .first()
        .map(|l| l.unit_price.currency.clone())
        .unwrap_or_else(|| shipping_fee.currency.clone());
    let mut subtotal = Money::zero(currency);
    for line in lines {
        subtotal = subtotal.checked_add(&line.total()?)?;
    }
    let total = subtotal
        .checked_add(shipping_fee)?
        .checked_add(handling_fee)?;
    Ok((subtotal, total))
}
