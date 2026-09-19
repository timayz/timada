use bitcode::{Decode, Encode};
use timada_core::{Money, MoneyError};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum InvoiceStatus {
    #[default]
    Draft,
    Issued,
    Voided,
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
