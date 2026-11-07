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
/// (`PaymentRefused`); the order exists once the provider holds the money on
/// the shopper's card, and the money is taken later.
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
        /// Who supplies some of the goods once the order is placed
        /// (`aliexpress`): the money is then taken once they are bought from
        /// it. `None` when the shop holds everything: taken at once.
        supplier: Option<String>,
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
    /// ([`OrderError::code`](crate::OrderError::code), or `payments_unavailable`).
    PaymentRefused {
        request_id: String,
        code: String,
        refused_by: String,
    },
    /// The payment of a request went back to the shopper, as its
    /// `PaymentReturnRequested` asked: the hold released, nothing taken, or
    /// the money refunded (`refund_id`) had it been taken meanwhile. No order
    /// was placed with it. `code` is the reason the return was asked with.
    /// Written by the dashboard, never a shopper.
    PaymentReturned {
        request_id: String,
        payment_id: String,
        code: String,
        /// The provider's id of the refund, when the money had been taken.
        refund_id: Option<String>,
        returned_by: String,
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
    /// The language the shopper browsed in, as its primary subtag (`fr`),
    /// written with the `CheckoutRequested` of the same request. Without
    /// it the shop mails the shopper in its default language.
    LanguageGiven {
        request_id: String,
        language: String,
        given_by: String,
    },
    /// The shopper ticked the box asking for the shop's news, written with
    /// the `CheckoutRequested` of the same request. Their address joins the
    /// list only once they confirm it, from the mail the order sends.
    NewsletterOptedIn {
        request_id: String,
        opted_in_by: String,
    },
    /// The IANA time zone the shopper's browser gave (`Europe/Paris`),
    /// written with the `CheckoutRequested` of the same request: the hours
    /// the shop's news keep for them, should they ask for it.
    TimeZoneGiven {
        request_id: String,
        time_zone: String,
        given_by: String,
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
    /// The payment of a request was cancelled with the provider before the
    /// shopper confirmed it: what the checkout held (the goods, the
    /// discount's use) had lapsed, so the card form it was opened for is
    /// closed rather than left to charge for goods it may not get. The
    /// shopper asks to pay again to go on. Written by the dashboard, never
    /// a shopper.
    PaymentCancelled {
        request_id: String,
        payment_id: String,
        cancelled_by: String,
    },
    /// The payment of a request is to go back to the shopper, before the
    /// provider is asked: it buys nothing. `code` says why: a second payment
    /// of a cart that is an order already (`duplicate_payment`), one that
    /// does not hold what the cart was priced at (`amount_mismatch`), or one
    /// authorized too late, what its checkout held (the goods, the discount's
    /// use) having lapsed and gone to someone else (`out_of_stock`,
    /// `discount_exhausted`, `discount_already_used`). Settled by the
    /// `PaymentReturned` that follows; meanwhile the payment makes no order,
    /// and whoever comes next (the provider telling again, its word that the
    /// hold went, the sweep) gives it back under this code. Written by the
    /// dashboard, never a shopper.
    PaymentReturnRequested {
        request_id: String,
        payment_id: String,
        code: String,
        requested_by: String,
    },
    /// The provider will not give the payment of a request back, for good
    /// (a disputed charge, say): `reason` is what it said. The return its
    /// `PaymentReturnRequested` asked for is settled, nothing more is tried,
    /// and the merchant settles it in the provider's own dashboard; the
    /// order of the cart, if it has one, shows it. Written by the dashboard,
    /// never a shopper.
    PaymentReturnRefused {
        request_id: String,
        payment_id: String,
        reason: String,
        refused_by: String,
    },
    /// A node took the request on: only `claimant` holds the goods and the
    /// code, opens the payment and answers, so the other nodes following the
    /// tenant leave it alone. Unanswered after
    /// [`CHECKOUT_CLAIM_SECS`](crate::CHECKOUT_CLAIM_SECS), the claim lapses
    /// and another node may take it on. Written by the dashboard, never a
    /// shopper.
    CheckoutClaimed {
        request_id: String,
        /// The node, as the dashboard names it (`node-2`): told the request
        /// again, after an error or a restart, it goes on under its claim.
        claimant: String,
        claimed_by: String,
    },
}

/// An order. Its id derives from the cart it was placed from
/// ([`order_id`](crate::order_id)); it exists once the provider holds its
/// payment on the shopper's card, and the money is taken (`Paid`) at once,
/// or once a supplier has the goods. Anything irreversible done outside the
/// store (taking the money, giving it back, ordering from the supplier) is
/// first asked for here, at the version read, so two of them never cross.
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
        /// What the shopper owes, in minor units: the sum of the lines, less
        /// the discount of a `Discounted` written with it.
        total_cents: u64,
        /// The payment holding it on the shopper's card.
        payment: Payment,
        /// Who supplies some of its goods once placed (`aliexpress`): the
        /// money is taken once they are bought from it. `None`: taken at
        /// once.
        supplier: Option<String>,
        placed_by: String,
    },
    /// The provider is about to be asked to take the held payment. Settled
    /// by the `Paid` or `CaptureFailed` that follows; until then the order
    /// is not cancelled.
    CaptureRequested {
        payment_id: String,
        amount_cents: u64,
        requested_by: String,
    },
    /// The held payment was taken: the order is paid.
    Paid {
        payment_id: String,
        amount_cents: u64,
        paid_by: String,
    },
    /// The provider would not take the held payment; `reason` is what it
    /// said. It may be asked again.
    CaptureFailed {
        payment_id: String,
        reason: String,
        failed_by: String,
    },
    /// The order is to be cancelled. The money is settled first, the hold
    /// released or what was taken refunded, and only then is the order
    /// `Cancelled`; nothing else happens to it meanwhile.
    CancellationRequested {
        reason: String,
        requested_by: String,
    },
    /// The hold on the shopper's card went back, nothing taken. Written
    /// with the `Cancelled` it makes, unless the provider dropped it on its
    /// own while the supplier may have the order: the order then stays open
    /// for the merchant to settle.
    PaymentReleased {
        payment_id: String,
        released_by: String,
    },
    /// The goods are about to be ordered from `supplier` (`aliexpress`),
    /// written at the version read just before it is asked, on every try:
    /// a cancellation landing first refuses it, one landing after finds it.
    /// From then on the supplier may have the order, and the shopper no
    /// longer cancels it. Begun again while the one before never said how
    /// it went, that one may be there: the order counts it uncertain.
    SupplierOrderStarted {
        supplier: String,
        started_by: String,
    },
    /// The supplier surely refused the order (it did not take it):
    /// the shopper may cancel again, unless an earlier order to it may have
    /// gone through (`SupplierOrderUncertain`).
    SupplierOrderRefused {
        supplier: String,
        reason: String,
        refused_by: String,
    },
    /// The order to `supplier` begun last never said how it went (the
    /// supplier did not answer, or what asked it stopped or could not record
    /// the answer): the supplier may have it, whatever a later order to it is
    /// told, until the merchant rules it out.
    SupplierOrderUncertain {
        supplier: String,
        reason: String,
        marked_by: String,
    },
    /// The merchant looked at the supplier and found no order there: it
    /// surely has none, and the shopper may cancel again.
    SupplierOrderRuledOut {
        supplier: String,
        ruled_out_by: String,
    },
    /// The goods left; `tracking` is a parcel number or link, if any.
    Fulfilled {
        tracking: Option<String>,
        fulfilled_by: String,
    },
    /// Final, once the money is settled: the hold released (written with
    /// `PaymentReleased`) or what was taken given back.
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
    /// The merchant asked for money to go back, before the provider is
    /// asked: the refund is made under `idempotency_key`, so asking the
    /// provider again (a retry after a failure, another node) gives the same
    /// refund. Settled by the `Refunded` that follows.
    RefundRequested {
        payment_id: String,
        amount_cents: u64,
        idempotency_key: String,
        requested_by: String,
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
    /// The language the shopper checked out in, as its primary subtag
    /// (`fr`), written with `Placed`; the shop's mails about the order go
    /// out in it. Without it they go out in the shop's default language.
    LanguageSet { language: String, set_by: String },
    /// The shopper asked for the shop's news at checkout, written with
    /// `Placed`: a confirmation link goes to the order's address.
    NewsletterOptInSet { set_by: String },
    /// The shopper's IANA time zone, written with `Placed` when their
    /// browser gave it.
    TimeZoneSet { time_zone: String, set_by: String },
}

/// The index `reference → order`: one stream per reference, with the
/// deterministic id [`order_reference_id`](crate::order_reference_id).
#[evento::aggregate(name = "timada-orders/OrderReference")]
pub enum OrderReference {
    Claimed {
        order_id: String,
    },
    /// Given back: the order it was claimed for was placed at the same
    /// moment elsewhere, under another reference.
    Released {
        released_by: String,
    },
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

/// A mail sent to the shopper of one order (`mail` names it: its
/// confirmation, its payment taken, its cancellation). One stream per order
/// and mail, with the id [`order_mail_id`](crate::order_mail_id): the node
/// that writes it first is the one that sends, so a shopper gets each mail
/// once however many nodes follow the tenant.
#[evento::aggregate(name = "timada-orders/OrderMail")]
pub enum OrderMail {
    Mailed {
        order_id: String,
        mail: String,
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
