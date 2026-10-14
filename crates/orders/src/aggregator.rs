//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it; what a shopper does is done by
//! [`STOREFRONT_ACTOR`](crate::STOREFRONT_ACTOR), since a guest has no
//! account to name.

use crate::value_object::{OrderLine, ShippingAddress};

/// What a shopper means to buy. Its id is a ULID the storefront keeps (a
/// cookie, typically). A line is a product, or one variant of it, and a
/// quantity; prices are not kept here, they are read when the order is
/// placed.
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
}

/// A placed order. Its id derives from the cart it was placed from
/// ([`order_id`](crate::order_id)).
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
        placed_by: String,
    },
    /// The merchant has been paid.
    Paid { paid_by: String },
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
}

/// The index `reference → order`: one stream per reference, with the
/// deterministic id [`order_reference_id`](crate::order_reference_id).
#[evento::aggregate(name = "timada-orders/OrderReference")]
pub enum OrderReference {
    Claimed { order_id: String },
}
