use timada_core::{Address, Money};

use crate::value_object::{DeliveryChoice, PaymentMode};

// The explicit name pins the on-disk identity: renaming the crate or the enum
// must never orphan stored events.
#[evento::aggregate(name = "timada-cart/Cart")]
pub enum Cart {
    /// A cart was started, optionally already tied to a signed-in customer.
    CartOpened { customer_id: Option<String> },

    /// A product was put in the cart with the price seen at that moment.
    CartLineAdded {
        product_id: String,
        name: String,
        quantity: u32,
        unit_price: Money,
        warranty_months: u16,
    },

    /// The quantity selector was changed.
    CartLineQuantityChanged { product_id: String, quantity: u32 },

    /// A line was removed.
    CartLineRemoved { product_id: String },

    /// The product's price moved since the line was added, and the cart was
    /// brought up to date: a cart is checked out at today's price, not at the
    /// price of the day it was filled — or saved.
    CartLineRepriced {
        product_id: String,
        unit_price: Money,
    },

    /// A promo code / voucher was typed in (advisory until checkout).
    PromoCodeApplied { code: String },

    /// The code was taken off the cart again.
    PromoCodeRemoved,

    /// The cart was saved under a name ("mes paniers sauvegardés").
    CartSaved { name: String },

    /// A signed-in customer took the cart as theirs — a guest who signed in,
    /// or a cart about to be saved. Saved carts are listed per owner.
    CartAssignedToCustomer { customer_id: String },

    /// A saved cart was made the current cart again.
    CartReopened,

    /// A saved cart was deleted from "mes paniers sauvegardés".
    CartDiscarded,

    /// The customer pressed "passer commande".
    CartCheckedOut {
        customer_id: String,
        delivery_address: Address,
        billing_address: Address,
        delivery: DeliveryChoice,
        payment_mode: PaymentMode,
    },
}
