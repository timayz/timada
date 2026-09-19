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

    /// A promo code / voucher was typed in (advisory until checkout).
    PromoCodeApplied { code: String },

    /// The code was taken off the cart again.
    PromoCodeRemoved,

    /// The cart was saved under a name ("mes paniers sauvegardés").
    CartSaved { name: String },

    /// The customer pressed "passer commande".
    CartCheckedOut {
        customer_id: String,
        delivery_address: Address,
        billing_address: Address,
        delivery: DeliveryChoice,
        payment_mode: PaymentMode,
    },
}
