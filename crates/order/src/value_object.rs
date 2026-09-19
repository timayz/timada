use bitcode::{Decode, Encode};
use timada_core::{Money, MoneyError};

/// An order line: the cart line frozen at checkout.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct OrderLine {
    pub product_id: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price: Money,
    pub warranty_months: u16,
}

impl OrderLine {
    pub fn total(&self) -> Result<Money, MoneyError> {
        self.unit_price.checked_mul(self.quantity)
    }
}

/// Sums the lines and adds the fees. `Err` on a currency mismatch or overflow.
pub fn order_total(
    lines: &[OrderLine],
    shipping_fee: &Money,
    handling_fee: &Money,
) -> Result<OrderTotals, MoneyError> {
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
    Ok(OrderTotals { subtotal, total })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderTotals {
    pub subtotal: Money,
    pub total: Money,
}

impl OrderTotals {
    /// The most a promo code or voucher may take off: the goods, never the
    /// fees — and always one minor unit short of the total, because the
    /// fulfillment saga has no path for an order with nothing to pay.
    pub fn max_discount(&self) -> Money {
        let minor = self.subtotal.minor.min(self.total.minor - 1).max(0);
        Money::new(minor, &self.total.currency)
    }
}

/// What the cart's code was: a promo code (a price reduction) or a voucher /
/// credit note (a balance spent on the order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum PromoKind {
    #[default]
    Discount,
    Voucher,
}

/// A code honoured on an order and what it takes off the total.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct OrderDiscount {
    pub code: String,
    pub kind: PromoKind,
    pub amount: Money,
}

/// Who sold the goods: the shop itself or a marketplace vendor.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum Seller {
    #[default]
    Ldlc,
    Marketplace {
        name: String,
    },
}

/// The delivery option picked at checkout (order's own copy of the cart's).
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct DeliveryChoice {
    pub method_code: String,
    pub pickup_store_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum PaymentMode {
    #[default]
    Card,
    Installments {
        count: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum OrderStatus {
    #[default]
    Placed,
    Paid,
    Shipped,
    Cancelled,
}

impl OrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OrderStatus::Placed => "placed",
            OrderStatus::Paid => "paid",
            OrderStatus::Shipped => "shipped",
            OrderStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct FulfillmentLine {
    pub product_id: String,
    pub quantity: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum FulfillmentStatus {
    #[default]
    ReservingStock,
    AwaitingPayment,
    AwaitingShipment,
    Completed,
    Compensated,
}
