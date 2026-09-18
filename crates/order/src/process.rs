//! Anti-corruption layer from the cart context: a `CartCheckedOut` fact is
//! translated into a `PlaceOrder` command. Not strict — it listens to one
//! event of a foreign aggregate.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_cart::aggregator::CartCheckedOut;
use timada_core::Money;

use crate::{
    command::{Command, PlaceOrder},
    error::OrderError,
    value_object::{DeliveryChoice, OrderLine, PaymentMode, Seller},
};

pub const ORDER_CHECKOUT_SUBSCRIPTION: &str = "order-checkout";

/// "Frais de dossier" charged on instalment plans, in minor units.
pub const INSTALLMENT_HANDLING_FEE_MINOR: i64 = 449;

pub fn order_checkout_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ORDER_CHECKOUT_SUBSCRIPTION).handler(place_order_on_cart_checked_out())
}

#[evento::subscription]
async fn place_order_on_cart_checked_out<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<CartCheckedOut>,
) -> anyhow::Result<()> {
    let cart_id = event.aggregate_id.to_owned();
    let Some(cart) = timada_cart::load_cart_details(ctx.executor, &cart_id).await? else {
        anyhow::bail!("cart {cart_id} checked out but its details cannot be loaded");
    };

    let currency = cart.subtotal.currency.clone();
    let shipping_fee =
        timada_shipping::shipping_fee(&event.data.delivery.method_code).ok_or_else(|| {
            OrderError::UnknownDeliveryMethod(event.data.delivery.method_code.clone())
        })?;
    let handling_fee = match event.data.payment_mode {
        timada_cart::PaymentMode::Card => Money::zero(&currency),
        timada_cart::PaymentMode::Installments { .. } => {
            Money::new(INSTALLMENT_HANDLING_FEE_MINOR, &currency)
        }
    };

    let cmd = PlaceOrder {
        cart_id: cart_id.clone(),
        customer_id: event.data.customer_id,
        seller: Seller::Ldlc,
        lines: cart.lines.into_iter().map(order_line).collect(),
        delivery_address: event.data.delivery_address,
        billing_address: event.data.billing_address,
        delivery: DeliveryChoice {
            method_code: event.data.delivery.method_code,
            pickup_store_id: event.data.delivery.pickup_store_id,
        },
        payment_mode: match event.data.payment_mode {
            timada_cart::PaymentMode::Card => PaymentMode::Card,
            timada_cart::PaymentMode::Installments { count } => PaymentMode::Installments { count },
        },
        shipping_fee,
        handling_fee,
        promo_code: cart.promo_code,
    };

    match Command(ctx.executor).place_order(cmd).await {
        Ok(_) => Ok(()),
        // Redelivery of the same checkout: the order already exists.
        Err(OrderError::AlreadyPlaced(_)) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn order_line(line: timada_cart::CartLine) -> OrderLine {
    OrderLine {
        product_id: line.product_id,
        name: line.name,
        quantity: line.quantity,
        unit_price: line.unit_price,
        warranty_months: line.warranty_months,
    }
}
