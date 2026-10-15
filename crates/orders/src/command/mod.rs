//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: a cart or an order is a handful of
//! events.

mod business_buyers;
mod cart;
mod checkout;
mod confirmation;
mod lifecycle;
mod payment_method;
mod shipping;
mod storefront;

use std::fmt;
use std::ops::Deref;

pub use cart::*;
pub use checkout::*;
pub use payment_method::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::{
    BillingAddressGiven, BillingAddressSet, BusinessBuyersAccepted, BusinessBuyersRefused,
    BuyerIdentified, Cancelled, CheckedOut, CheckoutRequested, Claimed, ConfirmationMailed,
    Created, DiscountApplied, DiscountGranted, DiscountRemoved, Discounted, Fulfilled, ItemAdded,
    ItemRemoved, MethodAdded, MethodChanged, MethodRemoved, PaymentRefused, PaymentRequested,
    PaymentsDisabled, PaymentsEnabled, Placed, QuantitySet, Refunded, StorefrontAddressCleared,
    StorefrontAddressSet,
};
use crate::value_object::{OrderLine, Payment, ShippingAddress, order_id};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// =============================================================================
// Cart
// =============================================================================

/// One line of a cart: a product, or one variant of it, and how many.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartLine {
    pub product_id: String,
    pub variant_id: Option<String>,
    pub quantity: u32,
}

impl CartLine {
    /// Whether this line is the one of that product and variant.
    pub fn is(&self, product_id: &str, variant_id: Option<&str>) -> bool {
        self.product_id == product_id && self.variant_id.as_deref() == variant_id
    }
}

/// A discount code taken off a cart's or an order's items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrderDiscount {
    pub discount_id: String,
    pub code: String,
    /// What the lines add up to before it.
    pub subtotal_cents: u64,
    /// What it takes off them.
    pub amount_cents: u64,
}

/// A payment opened with the provider for a cart: what the order will hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSnapshot {
    pub provider: String,
    pub payment_id: String,
    pub client_secret: String,
    pub publishable_key: String,
    pub lines: Vec<OrderLine>,
    pub currency: String,
    /// What is charged: the lines, less the discount.
    pub total_cents: u64,
    pub discount: Option<OrderDiscount>,
}

/// How a request to pay was answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaymentAnswer {
    /// A payment is open: the shopper's browser confirms it.
    Requested(PaymentSnapshot),
    /// It cannot go ahead; the code says why.
    Refused(String),
}

/// A shopper's request to pay for the cart, and its answer once there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRequest {
    pub request_id: String,
    pub email: String,
    pub shipping: ShippingAddress,
    pub note: Option<String>,
    pub expected_total_cents: Option<u64>,
    /// A business buyer's EU VAT number, as typed (normalized).
    pub vat_number: Option<String>,
    /// Where to bill, when not at `shipping`.
    pub billing: Option<ShippingAddress>,
    pub answer: Option<PaymentAnswer>,
}

impl CheckoutRequest {
    /// The open payment, when the request got one.
    pub fn payment(&self) -> Option<&PaymentSnapshot> {
        match &self.answer {
            Some(PaymentAnswer::Requested(payment)) => Some(payment),
            _ => None,
        }
    }
}

#[evento::projection(id = id, name = "timada-orders/CartState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CartState {
    pub id: String,
    /// In the order they were first added.
    pub lines: Vec<CartLine>,
    /// Every request to pay, oldest first.
    pub requests: Vec<CheckoutRequest>,
    /// The order it became, once checked out.
    pub order_id: Option<String>,
    /// The discount code the shopper entered (normalized), if any.
    pub discount_code: Option<String>,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl CartState {
    pub fn is_checked_out(&self) -> bool {
        self.order_id.is_some()
    }

    /// How many items, all lines together.
    pub fn item_count(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }

    pub fn line(&self, product_id: &str, variant_id: Option<&str>) -> Option<&CartLine> {
        self.lines
            .iter()
            .find(|line| line.is(product_id, variant_id))
    }

    pub fn request(&self, request_id: &str) -> Option<&CheckoutRequest> {
        self.requests.iter().find(|r| r.request_id == request_id)
    }

    /// The request whose open payment has that provider id.
    pub fn request_paid_by(&self, payment_id: &str) -> Option<&CheckoutRequest> {
        self.requests
            .iter()
            .find(|r| r.payment().is_some_and(|p| p.payment_id == payment_id))
    }

    /// Whether a payment is open for it (the shopper may be paying).
    pub fn is_paying(&self) -> bool {
        !self.is_checked_out() && self.requests.iter().any(|r| r.payment().is_some())
    }

    /// `open`, `paying` or `ordered`: what the read model stores.
    pub fn status(&self) -> &'static str {
        if self.is_checked_out() {
            "ordered"
        } else if self.is_paying() {
            "paying"
        } else {
            "open"
        }
    }
}

pub fn cart_projection<E: Executor>() -> Projection<E, CartState> {
    Projection::new::<crate::aggregator::Cart>()
        .handler(on_cart_created())
        .handler(on_item_added())
        .handler(on_quantity_set())
        .handler(on_item_removed())
        .handler(on_checked_out())
        .handler(on_checkout_requested())
        .handler(on_buyer_identified())
        .handler(on_billing_address_given())
        .handler(on_payment_requested())
        .handler(on_payment_refused())
        .handler(on_discount_applied())
        .handler(on_discount_removed())
        .handler(on_discount_granted())
        .strict()
}

#[evento::handler]
async fn on_cart_created(event: Event<Created>, cart: &mut CartState) -> anyhow::Result<()> {
    cart.id = event.aggregate_id.to_owned();
    cart.created_at = event.timestamp;
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_item_added(event: Event<ItemAdded>, cart: &mut CartState) -> anyhow::Result<()> {
    let variant_id = event.data.variant_id.as_deref();
    match cart
        .lines
        .iter_mut()
        .find(|line| line.is(&event.data.product_id, variant_id))
    {
        Some(line) => line.quantity += event.data.quantity,
        None => cart.lines.push(CartLine {
            product_id: event.data.product_id.to_owned(),
            variant_id: event.data.variant_id.to_owned(),
            quantity: event.data.quantity,
        }),
    }
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_quantity_set(event: Event<QuantitySet>, cart: &mut CartState) -> anyhow::Result<()> {
    let variant_id = event.data.variant_id.as_deref();
    if let Some(line) = cart
        .lines
        .iter_mut()
        .find(|line| line.is(&event.data.product_id, variant_id))
    {
        line.quantity = event.data.quantity;
    }
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_item_removed(event: Event<ItemRemoved>, cart: &mut CartState) -> anyhow::Result<()> {
    let variant_id = event.data.variant_id.as_deref();
    cart.lines
        .retain(|line| !line.is(&event.data.product_id, variant_id));
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_checked_out(event: Event<CheckedOut>, cart: &mut CartState) -> anyhow::Result<()> {
    cart.order_id = Some(event.data.order_id.to_owned());
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_checkout_requested(
    event: Event<CheckoutRequested>,
    cart: &mut CartState,
) -> anyhow::Result<()> {
    cart.requests.push(CheckoutRequest {
        request_id: event.data.request_id.to_owned(),
        email: event.data.email.to_owned(),
        shipping: event.data.shipping.to_owned(),
        note: event.data.note.to_owned(),
        expected_total_cents: event.data.expected_total_cents,
        vat_number: None,
        billing: None,
        answer: None,
    });
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_buyer_identified(
    event: Event<BuyerIdentified>,
    cart: &mut CartState,
) -> anyhow::Result<()> {
    if let Some(request) = cart
        .requests
        .iter_mut()
        .find(|request| request.request_id == event.data.request_id)
    {
        request.vat_number = Some(event.data.vat_number.to_owned());
    }
    Ok(())
}

#[evento::handler]
async fn on_billing_address_given(
    event: Event<BillingAddressGiven>,
    cart: &mut CartState,
) -> anyhow::Result<()> {
    if let Some(request) = cart
        .requests
        .iter_mut()
        .find(|request| request.request_id == event.data.request_id)
    {
        request.billing = Some(event.data.billing.to_owned());
    }
    Ok(())
}

#[evento::handler]
async fn on_payment_requested(
    event: Event<PaymentRequested>,
    cart: &mut CartState,
) -> anyhow::Result<()> {
    if let Some(request) = cart
        .requests
        .iter_mut()
        .find(|r| r.request_id == event.data.request_id)
    {
        request.answer = Some(PaymentAnswer::Requested(PaymentSnapshot {
            provider: event.data.provider.to_owned(),
            payment_id: event.data.payment_id.to_owned(),
            client_secret: event.data.client_secret.to_owned(),
            publishable_key: event.data.publishable_key.to_owned(),
            lines: event.data.lines.to_owned(),
            currency: event.data.currency.to_owned(),
            total_cents: event.data.total_cents,
            discount: None,
        }));
    }
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_payment_refused(
    event: Event<PaymentRefused>,
    cart: &mut CartState,
) -> anyhow::Result<()> {
    if let Some(request) = cart
        .requests
        .iter_mut()
        .find(|r| r.request_id == event.data.request_id)
    {
        request.answer = Some(PaymentAnswer::Refused(event.data.code.to_owned()));
    }
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_discount_applied(
    event: Event<DiscountApplied>,
    cart: &mut CartState,
) -> anyhow::Result<()> {
    cart.discount_code = Some(event.data.code.to_owned());
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_discount_removed(
    event: Event<DiscountRemoved>,
    cart: &mut CartState,
) -> anyhow::Result<()> {
    cart.discount_code = None;
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_discount_granted(
    event: Event<DiscountGranted>,
    cart: &mut CartState,
) -> anyhow::Result<()> {
    // Written right after the `PaymentRequested` it prices.
    if let Some(request) = cart
        .requests
        .iter_mut()
        .find(|r| r.request_id == event.data.request_id)
        && let Some(PaymentAnswer::Requested(payment)) = &mut request.answer
    {
        payment.discount = Some(OrderDiscount {
            discount_id: event.data.discount_id.to_owned(),
            code: event.data.code.to_owned(),
            subtotal_cents: event.data.subtotal_cents,
            amount_cents: event.data.amount_cents,
        });
    }
    Ok(())
}

// =============================================================================
// Order
// =============================================================================

/// Where an order stands. An order exists once paid; the merchant fulfils it,
/// or cancels it before it leaves (and the payment goes back).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OrderStatus {
    #[default]
    Paid,
    Fulfilled,
    Cancelled,
}

impl OrderStatus {
    pub const ALL: [Self; 3] = [Self::Paid, Self::Fulfilled, Self::Cancelled];

    /// The value stored in the read models.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Paid => "paid",
            Self::Fulfilled => "fulfilled",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }

    /// How a page words it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Paid => "Paid",
            Self::Fulfilled => "Fulfilled",
            Self::Cancelled => "Cancelled",
        }
    }
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[evento::projection(id = id, name = "timada-orders/OrderState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct OrderState {
    pub id: String,
    pub reference: String,
    pub cart_id: String,
    pub email: String,
    pub shipping: ShippingAddress,
    /// Where it is billed, when not at `shipping`.
    pub billing: Option<ShippingAddress>,
    pub note: Option<String>,
    pub lines: Vec<OrderLine>,
    pub currency: String,
    /// What was paid: the lines, less the discount.
    pub total_cents: u64,
    pub payment: Payment,
    /// The discount code it was paid with, if any.
    pub discount: Option<OrderDiscount>,
    /// What went back to the shopper, in minor units, all refunds together.
    pub refunded_cents: u64,
    /// The provider's ids of those refunds.
    pub refund_ids: Vec<String>,
    pub status: OrderStatus,
    pub tracking: Option<String>,
    pub cancel_reason: Option<String>,
    /// Unix seconds of `Placed`.
    pub placed_at: u64,
    pub fulfilled_at: Option<u64>,
    pub cancelled_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl OrderState {
    /// How many items, all lines together.
    pub fn item_count(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }

    /// What the lines add up to, before any discount.
    pub fn subtotal_cents(&self) -> u64 {
        self.lines.iter().map(OrderLine::amount_cents).sum()
    }

    /// Where the order is billed: its billing address, or else where it goes.
    pub fn billing_address(&self) -> &ShippingAddress {
        self.billing.as_ref().unwrap_or(&self.shipping)
    }
}

pub fn order_projection<E: Executor>() -> Projection<E, OrderState> {
    Projection::new::<crate::aggregator::Order>()
        .handler(on_placed())
        .handler(on_fulfilled())
        .handler(on_cancelled())
        .handler(on_refunded())
        .handler(on_discounted())
        .handler(on_billing_address_set())
        .strict()
}

#[evento::handler]
async fn on_placed(event: Event<Placed>, order: &mut OrderState) -> anyhow::Result<()> {
    order.id = event.aggregate_id.to_owned();
    order.reference = event.data.reference.to_owned();
    order.cart_id = event.data.cart_id.to_owned();
    order.email = event.data.email.to_owned();
    order.shipping = event.data.shipping.to_owned();
    order.note = event.data.note.to_owned();
    order.lines = event.data.lines.to_owned();
    order.currency = event.data.currency.to_owned();
    order.total_cents = event.data.total_cents;
    order.payment = event.data.payment.to_owned();
    order.status = OrderStatus::Paid;
    order.placed_at = event.timestamp;
    order.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_discounted(event: Event<Discounted>, order: &mut OrderState) -> anyhow::Result<()> {
    order.discount = Some(OrderDiscount {
        discount_id: event.data.discount_id.to_owned(),
        code: event.data.code.to_owned(),
        subtotal_cents: event.data.subtotal_cents,
        amount_cents: event.data.amount_cents,
    });
    Ok(())
}

#[evento::handler]
async fn on_billing_address_set(
    event: Event<BillingAddressSet>,
    order: &mut OrderState,
) -> anyhow::Result<()> {
    order.billing = Some(event.data.billing.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_refunded(event: Event<Refunded>, order: &mut OrderState) -> anyhow::Result<()> {
    order.refunded_cents += event.data.amount_cents;
    order.refund_ids.push(event.data.refund_id.to_owned());
    order.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_fulfilled(event: Event<Fulfilled>, order: &mut OrderState) -> anyhow::Result<()> {
    order.status = OrderStatus::Fulfilled;
    order.tracking = event.data.tracking.to_owned();
    order.fulfilled_at = Some(event.timestamp);
    order.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_cancelled(event: Event<Cancelled>, order: &mut OrderState) -> anyhow::Result<()> {
    order.status = OrderStatus::Cancelled;
    order.cancel_reason = Some(event.data.reason.to_owned());
    order.cancelled_at = Some(event.timestamp);
    order.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// OrderReference (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-orders/OrderReferenceState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct OrderReferenceState {
    pub id: String,
    pub order_id: String,
}

pub fn order_reference_projection<E: Executor>() -> Projection<E, OrderReferenceState> {
    Projection::new::<crate::aggregator::OrderReference>()
        .handler(on_reference_claimed())
        .strict()
}

#[evento::handler]
async fn on_reference_claimed(
    event: Event<Claimed>,
    index: &mut OrderReferenceState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.order_id = event.data.order_id.to_owned();
    Ok(())
}

// =============================================================================
// PaymentMethod
// =============================================================================

#[evento::projection(id = id, name = "timada-orders/PaymentMethodState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct PaymentMethodState {
    pub id: String,
    /// `None` while the tenant takes no payments.
    pub setup: Option<PaymentSetup>,
}

pub fn payment_method_projection<E: Executor>() -> Projection<E, PaymentMethodState> {
    Projection::new::<crate::aggregator::PaymentMethod>()
        .handler(on_payments_enabled())
        .handler(on_payments_disabled())
        .strict()
}

#[evento::handler]
async fn on_payments_enabled(
    event: Event<PaymentsEnabled>,
    state: &mut PaymentMethodState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.setup = Some(PaymentSetup {
        provider: event.data.provider.to_owned(),
        publishable_key: event.data.publishable_key.to_owned(),
    });
    Ok(())
}

#[evento::handler]
async fn on_payments_disabled(
    event: Event<PaymentsDisabled>,
    state: &mut PaymentMethodState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.setup = None;
    Ok(())
}

// =============================================================================
// BusinessBuyers
// =============================================================================

#[evento::projection(id = id, name = "timada-orders/BusinessBuyersState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct BusinessBuyersState {
    pub id: String,
    pub accepted: bool,
}

pub fn business_buyers_projection<E: Executor>() -> Projection<E, BusinessBuyersState> {
    Projection::new::<crate::aggregator::BusinessBuyers>()
        .handler(on_business_buyers_accepted())
        .handler(on_business_buyers_refused())
        .strict()
}

#[evento::handler]
async fn on_business_buyers_accepted(
    event: Event<BusinessBuyersAccepted>,
    state: &mut BusinessBuyersState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.accepted = true;
    Ok(())
}

#[evento::handler]
async fn on_business_buyers_refused(
    event: Event<BusinessBuyersRefused>,
    state: &mut BusinessBuyersState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.accepted = false;
    Ok(())
}

// =============================================================================
// ShippingMethods
// =============================================================================

/// One way the shop ships, as the merchant last described it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShippingMethod {
    pub id: String,
    pub name: String,
    pub price_cents: u64,
    pub currency: String,
    /// Two-letter codes; empty means every country.
    pub countries: Vec<String>,
    /// Free once the goods, less any discount, reach this amount.
    pub free_above_cents: Option<u64>,
}

impl ShippingMethod {
    /// Whether the method ships to that country.
    pub fn covers(&self, country: &str) -> bool {
        self.countries.is_empty() || self.countries.iter().any(|c| c == country)
    }

    /// What shipping goods worth `goods_cents` (less any discount) costs.
    pub fn price_for(&self, goods_cents: u64) -> u64 {
        match self.free_above_cents {
            Some(threshold) if goods_cents >= threshold => 0,
            _ => self.price_cents,
        }
    }
}

#[evento::projection(id = id, name = "timada-orders/ShippingMethodsState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ShippingMethodsState {
    pub id: String,
    /// In the order they were added.
    pub methods: Vec<ShippingMethod>,
}

impl ShippingMethodsState {
    pub fn method(&self, method_id: &str) -> Option<&ShippingMethod> {
        self.methods.iter().find(|method| method.id == method_id)
    }
}

pub fn shipping_methods_projection<E: Executor>() -> Projection<E, ShippingMethodsState> {
    Projection::new::<crate::aggregator::ShippingMethods>()
        .handler(on_method_added())
        .handler(on_method_changed())
        .handler(on_method_removed())
        .strict()
}

#[evento::handler]
async fn on_method_added(
    event: Event<MethodAdded>,
    state: &mut ShippingMethodsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.methods.push(ShippingMethod {
        id: event.data.method_id.clone(),
        name: event.data.name.clone(),
        price_cents: event.data.price_cents,
        currency: event.data.currency.clone(),
        countries: event.data.countries.clone(),
        free_above_cents: event.data.free_above_cents,
    });
    Ok(())
}

#[evento::handler]
async fn on_method_changed(
    event: Event<MethodChanged>,
    state: &mut ShippingMethodsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    if let Some(method) = state
        .methods
        .iter_mut()
        .find(|method| method.id == event.data.method_id)
    {
        method.name = event.data.name.clone();
        method.price_cents = event.data.price_cents;
        method.currency = event.data.currency.clone();
        method.countries = event.data.countries.clone();
        method.free_above_cents = event.data.free_above_cents;
    }
    Ok(())
}

#[evento::handler]
async fn on_method_removed(
    event: Event<MethodRemoved>,
    state: &mut ShippingMethodsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state
        .methods
        .retain(|method| method.id != event.data.method_id);
    Ok(())
}

// =============================================================================
// Storefront
// =============================================================================

#[evento::projection(id = id, name = "timada-orders/StorefrontState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct StorefrontState {
    pub id: String,
    /// Absolute, without a trailing `/`.
    pub url: Option<String>,
}

pub fn storefront_projection<E: Executor>() -> Projection<E, StorefrontState> {
    Projection::new::<crate::aggregator::Storefront>()
        .handler(on_storefront_address_set())
        .handler(on_storefront_address_cleared())
        .strict()
}

#[evento::handler]
async fn on_storefront_address_set(
    event: Event<StorefrontAddressSet>,
    state: &mut StorefrontState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.url = Some(event.data.url.clone());
    Ok(())
}

#[evento::handler]
async fn on_storefront_address_cleared(
    event: Event<StorefrontAddressCleared>,
    state: &mut StorefrontState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.url = None;
    Ok(())
}

// =============================================================================
// OrderConfirmation
// =============================================================================

#[evento::projection(id = id, name = "timada-orders/OrderConfirmationState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct OrderConfirmationState {
    pub id: String,
    pub email: String,
    /// Unix seconds.
    pub mailed_at: u64,
}

pub fn order_confirmation_projection<E: Executor>() -> Projection<E, OrderConfirmationState> {
    Projection::new::<crate::aggregator::OrderConfirmation>()
        .handler(on_confirmation_mailed())
        .strict()
}

#[evento::handler]
async fn on_confirmation_mailed(
    event: Event<ConfirmationMailed>,
    state: &mut OrderConfirmationState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.email = event.data.email.clone();
    state.mailed_at = event.timestamp;
    Ok(())
}

// =============================================================================
// Loads
// =============================================================================

impl<E: Executor> Command<E> {
    pub async fn load_cart(&self, cart_id: &str) -> anyhow::Result<Option<CartState>> {
        cart_projection().load(cart_id).execute(&self.0).await
    }

    pub async fn load_order(&self, order_id: &str) -> anyhow::Result<Option<OrderState>> {
        order_projection().load(order_id).execute(&self.0).await
    }

    pub async fn load_payment_method(&self) -> anyhow::Result<Option<PaymentMethodState>> {
        payment_method_projection()
            .load(crate::value_object::payment_method_id())
            .execute(&self.0)
            .await
    }

    pub async fn load_storefront(&self) -> anyhow::Result<Option<StorefrontState>> {
        storefront_projection()
            .load(crate::value_object::storefront_id())
            .execute(&self.0)
            .await
    }

    /// The confirmation mailed for an order, if one was.
    pub async fn load_order_confirmation(
        &self,
        order_id: &str,
    ) -> anyhow::Result<Option<OrderConfirmationState>> {
        order_confirmation_projection()
            .load(crate::value_object::order_confirmation_id(order_id))
            .execute(&self.0)
            .await
    }

    pub async fn load_business_buyers(&self) -> anyhow::Result<Option<BusinessBuyersState>> {
        business_buyers_projection()
            .load(crate::value_object::business_buyers_id())
            .execute(&self.0)
            .await
    }

    pub async fn load_shipping_methods(&self) -> anyhow::Result<Option<ShippingMethodsState>> {
        shipping_methods_projection()
            .load(crate::value_object::shipping_methods_id())
            .execute(&self.0)
            .await
    }

    /// The order a cart was placed as, if it was.
    pub async fn load_order_of_cart(&self, cart_id: &str) -> anyhow::Result<Option<OrderState>> {
        self.load_order(&order_id(cart_id)).await
    }

    /// The cart, or `CartNotFound`.
    pub(crate) async fn require_cart(
        &self,
        cart_id: &str,
    ) -> Result<CartState, crate::OrdersError> {
        self.load_cart(cart_id)
            .await?
            .ok_or(crate::OrdersError::CartNotFound)
    }

    /// The order, or `OrderNotFound`.
    pub(crate) async fn require_order(
        &self,
        order_id: &str,
    ) -> Result<OrderState, crate::OrdersError> {
        self.load_order(order_id)
            .await?
            .ok_or(crate::OrdersError::OrderNotFound)
    }
}
