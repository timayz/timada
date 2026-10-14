//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it; what a shopper does is done by
//! [`STOREFRONT_ACTOR`](crate::STOREFRONT_ACTOR), since a guest has no
//! account to name.

use crate::value_object::{OrderLine, Payment, ShippingAddress};

/// What a shopper means to buy. Its id is a ULID the storefront keeps (a
/// cookie, typically). A line is a product, or one variant of it, and a
/// quantity; prices are not kept on the lines, they are read when a payment
/// is asked for. Paying goes in turns: the shopper asks to pay
/// (`CheckoutRequested`), the dashboard prices the cart and opens a payment
/// with the provider (`PaymentRequested`) or says why it cannot
/// (`PaymentRefused`); the order exists once the provider confirms.
#[evento::aggregate(name = "timada-orders/Cart")]
pub enum Cart {
    Created {
        created_by: String,
    },
    /// Adds `quantity` to the line of that product and variant, creating it.
    ItemAdded {
        product_id: String,
        variant_id: Option<String>,
        quantity: u32,
        added_by: String,
    },
    /// The line's new quantity, never zero (that is `ItemRemoved`).
    QuantitySet {
        product_id: String,
        variant_id: Option<String>,
        quantity: u32,
        set_by: String,
    },
    ItemRemoved {
        product_id: String,
        variant_id: Option<String>,
        removed_by: String,
    },
    /// Final: the cart became that order and takes no more changes.
    CheckedOut {
        order_id: String,
        checked_out_by: String,
    },
    /// The shopper asks to pay for the cart as it stands, shipping there.
    CheckoutRequested {
        /// Answered by the `PaymentRequested` or `PaymentRefused` carrying it.
        request_id: String,
        email: String,
        shipping: ShippingAddress,
        note: Option<String>,
        /// The total the shopper was shown; a cart priced otherwise is
        /// refused rather than charged unseen.
        expected_total_cents: Option<u64>,
        requested_by: String,
    },
    /// A payment is open with the provider for the cart priced as `lines`:
    /// what the order will hold once it is paid.
    PaymentRequested {
        request_id: String,
        /// `stripe`.
        provider: String,
        /// The provider's id of the payment (a PaymentIntent's `pi_…`).
        payment_id: String,
        /// What the shopper's browser confirms the payment with.
        client_secret: String,
        /// The merchant's public key the browser loads the card form with.
        publishable_key: String,
        lines: Vec<OrderLine>,
        currency: String,
        total_cents: u64,
        requested_by: String,
    },
    /// A request to pay that cannot go ahead; `code` says why
    /// ([`OrdersError::code`](crate::OrdersError::code), or `payments_unavailable`).
    PaymentRefused {
        request_id: String,
        code: String,
        refused_by: String,
    },
}

/// A paid order. Its id derives from the cart it was placed from
/// ([`order_id`](crate::order_id)); it exists only once its payment went
/// through.
#[evento::aggregate(name = "timada-orders/Order")]
pub enum Order {
    /// Everything about the sale, copied at the moment it was made.
    Placed {
        /// The short code a shopper and the merchant read (`K7Q4-MZP2`).
        reference: String,
        cart_id: String,
        email: String,
        shipping: ShippingAddress,
        /// What the shopper wrote for the merchant, if anything.
        note: Option<String>,
        lines: Vec<OrderLine>,
        /// The ISO-4217 code every line is priced in.
        currency: String,
        /// The sum of the lines, in minor units.
        total_cents: u64,
        /// The payment that paid for it.
        payment: Payment,
        placed_by: String,
    },
    /// The goods left; `tracking` is a parcel number or link, if any.
    Fulfilled {
        tracking: Option<String>,
        fulfilled_by: String,
    },
    /// Final.
    Cancelled {
        reason: String,
        cancelled_by: String,
    },
    /// Money went back to the shopper through the provider.
    Refunded {
        payment_id: String,
        /// The provider's id of the refund.
        refund_id: String,
        amount_cents: u64,
        refunded_by: String,
    },
}

/// The index `reference → order`: one stream per reference, with the
/// deterministic id [`order_reference_id`](crate::order_reference_id).
#[evento::aggregate(name = "timada-orders/OrderReference")]
pub enum OrderReference {
    Claimed { order_id: String },
}
