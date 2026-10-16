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
    /// The shopper buys as a business with this VAT number (`DE123456789`),
    /// written with the `CheckoutRequested` of the same request. Whether it
    /// is valid is checked when the order is invoiced.
    BuyerIdentified {
        request_id: String,
        vat_number: String,
        identified_by: String,
    },
    /// A request to pay that cannot go ahead; `code` says why
    /// ([`OrdersError::code`](crate::OrdersError::code), or `payments_unavailable`).
    PaymentRefused {
        request_id: String,
        code: String,
        refused_by: String,
    },
    /// The shopper enters a discount code (normalized), replacing any other:
    /// a cart takes one. Whether it takes anything off is decided each time
    /// the cart is priced.
    DiscountApplied {
        code: String,
        applied_by: String,
    },
    DiscountRemoved {
        removed_by: String,
    },
    /// The discount the payment of a request is priced with, written with
    /// its `PaymentRequested`: the lines add up to `subtotal_cents`, the
    /// payment's total is that less `amount_cents`.
    DiscountGranted {
        request_id: String,
        discount_id: String,
        code: String,
        subtotal_cents: u64,
        amount_cents: u64,
        granted_by: String,
    },
    /// The shopper is billed at another address than the one the goods go
    /// to, written with the `CheckoutRequested` of the same request. No
    /// phone is kept: nobody rings a billing address.
    BillingAddressGiven {
        request_id: String,
        billing: ShippingAddress,
        given_by: String,
    },
    /// The shipping method the shopper picked, written with the
    /// `CheckoutRequested` of the same request. A shop without methods
    /// ships for free, and none is picked.
    ShippingMethodChosen {
        request_id: String,
        method_id: String,
        chosen_by: String,
    },
    /// What shipping the payment of a request is priced with, written with
    /// its `PaymentRequested`: the payment's total is the lines, less any
    /// discount, plus `amount_cents`.
    ShippingGranted {
        request_id: String,
        method_id: String,
        name: String,
        amount_cents: u64,
        granted_by: String,
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
        /// What was paid, in minor units: the sum of the lines, less the
        /// discount of a `Discounted` written with it.
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
    /// The order was paid with a discount code, written with `Placed`: its
    /// lines add up to `subtotal_cents`, `amount_cents` came off them.
    Discounted {
        discount_id: String,
        code: String,
        subtotal_cents: u64,
        amount_cents: u64,
        discounted_by: String,
    },
    /// Money went back to the shopper through the provider.
    Refunded {
        payment_id: String,
        /// The provider's id of the refund.
        refund_id: String,
        amount_cents: u64,
        refunded_by: String,
    },
    /// The order is billed at another address than its `shipping`, written
    /// with `Placed`; without it, the invoice goes to the shipping address.
    BillingAddressSet {
        billing: ShippingAddress,
        set_by: String,
    },
    /// What shipping the order was paid with, written with `Placed`: its
    /// total is the lines, less any discount, plus `amount_cents`.
    ShippingCharged {
        method_id: String,
        name: String,
        amount_cents: u64,
        charged_by: String,
    },
}

/// The index `reference → order`: one stream per reference, with the
/// deterministic id [`order_reference_id`](crate::order_reference_id).
#[evento::aggregate(name = "timada-orders/OrderReference")]
pub enum OrderReference {
    Claimed { order_id: String },
}

/// How the tenant's shoppers pay, as their browsers need to know it before
/// any payment is open: the provider and the merchant's public key. One
/// stream per tenant store, with the id [`payment_method_id`](crate::payment_method_id).
/// The dashboard keeps it in step with the merchant's account; the secrets
/// never come here.
#[evento::aggregate(name = "timada-orders/PaymentMethod")]
pub enum PaymentMethod {
    /// Replaces what was there.
    PaymentsEnabled {
        /// `stripe`.
        provider: String,
        /// Public: what a shopper's browser loads the payment form with.
        publishable_key: String,
        enabled_by: String,
    },
    PaymentsDisabled {
        disabled_by: String,
    },
}

/// Whether the tenant sells to businesses: a shopper who gives an EU VAT
/// number is invoiced without VAT under reverse charge. One stream per
/// tenant store, with the id [`business_buyers_id`](crate::business_buyers_id);
/// no stream means the shop does not.
#[evento::aggregate(name = "timada-orders/BusinessBuyers")]
pub enum BusinessBuyers {
    BusinessBuyersAccepted { accepted_by: String },
    BusinessBuyersRefused { refused_by: String },
}

/// Where the tenant's storefront lives, so the mails the dashboard sends a
/// shopper can link back to it. One stream per tenant store, with the id
/// [`storefront_id`](crate::storefront_id); no stream means no link.
#[evento::aggregate(name = "timada-orders/Storefront")]
pub enum Storefront {
    /// `url` is absolute, without a trailing `/` (`https://shop.example`).
    StorefrontAddressSet {
        url: String,
        set_by: String,
    },
    StorefrontAddressCleared {
        cleared_by: String,
    },
}

/// The confirmation mailed to the shopper of one order. One stream per
/// order, with the id [`order_confirmation_id`](crate::order_confirmation_id):
/// the node that writes it first is the one that sends, so a shopper gets
/// one mail however many nodes follow the tenant.
#[evento::aggregate(name = "timada-orders/OrderConfirmation")]
pub enum OrderConfirmation {
    ConfirmationMailed {
        order_id: String,
        email: String,
        mailed_by: String,
    },
}

/// How the tenant ships: the methods a shopper picks one of at checkout.
/// One stream per tenant store, with the id
/// [`shipping_methods_id`](crate::shipping_methods_id), so a quote reads them
/// all in one load; no stream (or none left) means shipping is included.
/// `countries` empty means every country.
#[evento::aggregate(name = "timada-orders/ShippingMethods")]
pub enum ShippingMethods {
    MethodAdded {
        method_id: String,
        name: String,
        price_cents: u64,
        currency: String,
        countries: Vec<String>,
        /// Free once the goods, less any discount, reach this amount.
        free_above_cents: Option<u64>,
        added_by: String,
    },
    /// Replaces everything the method was.
    MethodChanged {
        method_id: String,
        name: String,
        price_cents: u64,
        currency: String,
        countries: Vec<String>,
        free_above_cents: Option<u64>,
        changed_by: String,
    },
    MethodRemoved {
        method_id: String,
        removed_by: String,
    },
}
