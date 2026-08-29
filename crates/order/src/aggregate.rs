//! The `Order` aggregate — and the fulfillment saga's state.
//!
//! An order is the customer's copy of the deal: what was bought, at what price,
//! where it ships. Everything after `OrderPlaced` is a fact reported by another
//! context (payment, dropship, shipping) and recorded here by the saga, which
//! is why the later events carry only the foreign aggregate id they came from.
//!
//! `OrderPlaced` snapshots the cart lines and the total rather than pointing at
//! the cart: a re-priced catalog must never rewrite what the customer agreed
//! to pay. The `total` is the one piece of derivable data kept on an event on
//! purpose — it is the amount the payment provider is asked to charge, so it
//! must be the number the customer saw, not a number recomputed later from
//! lines that could round differently.
//!
//! Prices are tax-inclusive, so the tax amounts are snapshotted for the same
//! reason and then some: an invoice reprinted years later must show the rate
//! that applied on the day of the sale, not today's.

use timada_core::Money;

/// Where the parcel goes. Nested in `OrderPlaced`, hence the bitcode derives.
///
/// One flat, unvalidated shape: address formats are country-specific and the
/// framework has no business rejecting a valid Japanese address for not looking
/// French. Checkout only refuses empty fields.
#[derive(Debug, Clone, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub struct Address {
    pub full_name: String,
    pub street: String,
    pub city: String,
    pub postal_code: String,
    pub country: String,
}

/// One product on an order, at the price and title the customer accepted.
///
/// `supplier_id` and `supplier_product_ref` ride along from the cart line so
/// the saga can group the order by supplier without asking the catalog again —
/// the product may have been archived or re-imported since.
///
/// `unit_price` is the tax-inclusive price of one unit, exactly as the
/// storefront showed it. `net` and `tax` are *line* amounts covering all
/// `quantity` units, so `net + tax == line_total()`.
#[derive(Debug, Clone, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub struct OrderLine {
    pub product_id: String,
    pub title: String,
    pub unit_price: Money,
    pub supplier_id: String,
    pub supplier_product_ref: String,
    pub quantity: u32,
    /// Rate applied to this line, in basis points (2000 = 20 %).
    pub tax_rate_bps: u32,
    pub net: Money,
    pub tax: Money,
}

impl OrderLine {
    /// What this line costs, tax included: unit price times quantity.
    pub fn line_total(&self) -> Money {
        self.unit_price.multiply(self.quantity)
    }
}

#[evento::aggregate]
pub enum Order {
    /// Checkout succeeded: the cart became an order nobody can edit any more.
    ///
    /// `total` is what the customer pays; `total_net` and `total_tax` are the
    /// assessed split of it, both snapshotted so an invoice never has to
    /// re-derive tax from rates that may have moved since.
    OrderPlaced {
        cart_id: String,
        /// The signed-in customer this order belongs to; `None` for guests.
        /// Snapshotted at checkout — logging in later never rewrites history.
        customer_id: Option<String>,
        email: String,
        shipping_address: Address,
        lines: Vec<OrderLine>,
        total: Money,
        total_net: Money,
        total_tax: Money,
    },
    /// The payment context captured the charge.
    OrderPaid { payment_id: String },
    /// A supplier accepted its share of the lines. Multi-supplier orders emit
    /// this once per supplier; the status only moves on the first one.
    OrderForwardedToSupplier { supplier_order_id: String },
    /// A parcel is on its way.
    OrderShipped { tracking_number: String },
    /// The customer has it. Terminal.
    OrderDelivered,
    /// Fulfillment gave up — a declined charge, or a supplier that refused.
    /// Terminal, and only reachable before anything has shipped.
    OrderCancelled { reason: String },
}
