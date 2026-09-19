//! The expanded order in "Historique de vos commandes": lines, delivery,
//! payment mode, fees and totals. Executor-backed snapshots via bitcode.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::{Address, Money};
use timada_tax::{TaxTreatment, VatLine};

use crate::{
    aggregator::{
        Order, OrderCancelled, OrderConfirmationResent, OrderDiscountApplied, OrderNumberAssigned,
        OrderPaid, OrderPlaced, OrderSettled, OrderShipped, OrderTaxed,
    },
    value_object::{
        DeliveryChoice, OrderDiscount, OrderLine, OrderStatus, PaymentMode, Seller, order_total,
    },
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct OrderDetailsView {
    pub id: String,
    /// "C2026-000042"; `None` for an order placed without a number.
    pub order_number: Option<String>,
    pub cart_id: String,
    pub customer_id: String,
    pub seller: Seller,
    /// Unix seconds of `OrderPlaced`.
    pub placed_at: u64,
    pub status: OrderStatus,
    pub lines: Vec<OrderLine>,
    pub delivery_address: Address,
    pub billing_address: Address,
    pub delivery: DeliveryChoice,
    pub payment_mode: PaymentMode,
    pub shipping_fee: Money,
    pub handling_fee: Money,
    pub subtotal: Money,
    /// The honoured code and what it takes off; `total` is already net of it.
    pub discount: Option<OrderDiscount>,
    /// How the order was taxed; `None` for orders older than tax zones.
    pub tax: Option<OrderTaxSummary>,
    pub total: Money,
    /// The code typed in the cart, honoured or not.
    pub promo_code: Option<String>,
    pub payment_id: Option<String>,
    pub shipment_id: Option<String>,
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    /// Unix seconds of `OrderShipped` ("Expédiée le ...").
    pub shipped_at: Option<u64>,
    pub cancelled_reason: Option<String>,
    pub confirmation_resent_count: u32,
}

pub fn create_projection<E: Executor>() -> Projection<E, OrderDetailsView> {
    Projection::new::<Order>()
        .handler(on_order_placed())
        .handler(on_order_discount_applied())
        .handler(on_order_paid())
        .handler(on_order_settled())
        .handler(on_order_shipped())
        .handler(on_order_cancelled())
        .handler(on_order_confirmation_resent())
        .handler(on_order_number_assigned())
        .handler(on_order_taxed())
        // The view gained `order_number`, then `tax`: snapshots taken with a
        // previous shape must not be decoded.
        .revision(2)
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<OrderDetailsView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_order_placed(
    event: Event<OrderPlaced>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    let totals = order_total(
        &event.data.lines,
        &event.data.shipping_fee,
        &event.data.handling_fee,
    )?;
    row.id = event.aggregate_id.to_owned();
    row.placed_at = event.timestamp;
    row.status = OrderStatus::Placed;
    row.cart_id = event.data.cart_id;
    row.customer_id = event.data.customer_id;
    row.seller = event.data.seller;
    row.lines = event.data.lines;
    row.delivery_address = event.data.delivery_address;
    row.billing_address = event.data.billing_address;
    row.delivery = event.data.delivery;
    row.payment_mode = event.data.payment_mode;
    row.shipping_fee = event.data.shipping_fee;
    row.handling_fee = event.data.handling_fee;
    row.subtotal = totals.subtotal;
    row.total = totals.total;
    row.promo_code = event.data.promo_code;
    Ok(())
}

/// The zone an order was taxed in and the VAT inside what was charged.
#[derive(Debug, Clone, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub struct OrderTaxSummary {
    pub zone_code: String,
    pub treatment: TaxTreatment,
    pub vat_lines: Vec<VatLine>,
}

impl OrderTaxSummary {
    /// The VAT of all rates together.
    pub fn vat_total(&self) -> Result<Money, timada_core::MoneyError> {
        let currency = self
            .vat_lines
            .first()
            .map_or_else(|| Money::EUR.to_owned(), |l| l.vat.currency.clone());
        let mut total = Money::zero(currency);
        for line in &self.vat_lines {
            total = total.checked_add(&line.vat)?;
        }
        Ok(total)
    }
}

#[evento::handler]
async fn on_order_taxed(
    event: Event<OrderTaxed>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.tax = Some(OrderTaxSummary {
        zone_code: event.data.zone_code,
        treatment: event.data.treatment,
        vat_lines: event.data.vat_lines,
    });
    Ok(())
}

impl OrderDetailsView {
    /// What to call the order in front of people: its number, or its id when
    /// it has none.
    pub fn display_number(&self) -> &str {
        self.order_number.as_deref().unwrap_or(&self.id)
    }
}

#[evento::handler]
async fn on_order_number_assigned(
    event: Event<OrderNumberAssigned>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.order_number = Some(event.data.order_number);
    Ok(())
}

#[evento::handler]
async fn on_order_discount_applied(
    event: Event<OrderDiscountApplied>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.total = row.total.checked_sub(&event.data.amount)?;
    row.discount = Some(OrderDiscount {
        code: event.data.code,
        kind: event.data.kind,
        amount: event.data.amount,
    });
    Ok(())
}

#[evento::handler]
async fn on_order_paid(event: Event<OrderPaid>, row: &mut OrderDetailsView) -> anyhow::Result<()> {
    row.status = OrderStatus::Paid;
    row.payment_id = Some(event.data.payment_id);
    Ok(())
}

#[evento::handler]
async fn on_order_settled(
    _event: Event<OrderSettled>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.status = OrderStatus::Paid;
    Ok(())
}

#[evento::handler]
async fn on_order_shipped(
    event: Event<OrderShipped>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.status = OrderStatus::Shipped;
    row.shipped_at = Some(event.timestamp);
    row.shipment_id = Some(event.data.shipment_id);
    row.carrier = Some(event.data.carrier);
    row.tracking_number = Some(event.data.tracking_number);
    Ok(())
}

#[evento::handler]
async fn on_order_cancelled(
    event: Event<OrderCancelled>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.status = OrderStatus::Cancelled;
    row.cancelled_reason = Some(event.data.reason);
    Ok(())
}

#[evento::handler]
async fn on_order_confirmation_resent(
    _event: Event<OrderConfirmationResent>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.confirmation_resent_count += 1;
    Ok(())
}
