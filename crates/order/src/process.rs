//! Anti-corruption layer from the cart context: a `CartCheckedOut` fact is
//! translated into a `PlaceOrder` command, after the cart's code — if any —
//! was redeemed with the promotion context, so the order is born with the
//! price it will be paid at. The mirror subscription gives the code back when
//! the order is cancelled. Neither is strict — each listens to one event.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_cart::aggregator::CartCheckedOut;
use timada_core::Money;
use timada_promotion::{CodeKind, PromotionError};

use crate::{
    aggregator::OrderCancelled,
    command::{Command, PlaceOrder, order_id},
    error::OrderError,
    query::load_order_details,
    value_object::{
        DeliveryChoice, OrderDiscount, OrderLine, OrderTotals, PaymentMode, PromoKind, Seller,
        order_total,
    },
};

pub const ORDER_CHECKOUT_SUBSCRIPTION: &str = "order-checkout";
pub const ORDER_PROMO_RELEASE_SUBSCRIPTION: &str = "order-promo-release";

/// "Frais de dossier" charged on instalment plans, in minor units.
pub const INSTALLMENT_HANDLING_FEE_MINOR: i64 = 449;

/// Needs the `SqlitePool` as subscription data: promo-code redemption caps
/// are counted in SQL.
pub fn order_checkout_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ORDER_CHECKOUT_SUBSCRIPTION).handler(place_order_on_cart_checked_out())
}

/// Needs the `SqlitePool` as subscription data, like the checkout one.
pub fn order_promo_release_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ORDER_PROMO_RELEASE_SUBSCRIPTION)
        .handler(release_code_on_order_cancelled())
}

fn promotion<'a, E: Executor>(
    ctx: &'a Context<'_, E>,
) -> anyhow::Result<timada_promotion::Command<'a, E>> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    Ok(timada_promotion::Command {
        executor: ctx.executor,
        db,
    })
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

    let lines: Vec<OrderLine> = cart.lines.into_iter().map(order_line).collect();
    let totals = order_total(&lines, &shipping_fee, &handling_fee)?;
    let discount = match cart.promo_code.as_deref() {
        Some(code) => redeem_code(ctx, code, &order_id(&cart_id), &totals).await?,
        None => None,
    };

    let cmd = PlaceOrder {
        cart_id: cart_id.clone(),
        customer_id: event.data.customer_id,
        seller: Seller::Ldlc,
        lines,
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
        discount,
    };

    match Command(ctx.executor).place_order(cmd).await {
        Ok(_) => Ok(()),
        // Redelivery of the same checkout: the order already exists.
        Err(OrderError::AlreadyPlaced(_)) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Redeems the cart's code for the order about to be placed. Idempotent per
/// order id, so a redelivery gets the same answer. A code the promotion
/// context refuses (the storefront checks it, but a cap can fill up in
/// between) is logged and the order is placed at full price rather than lost.
async fn redeem_code<E: Executor>(
    ctx: &Context<'_, E>,
    code: &str,
    order_id: &str,
    totals: &OrderTotals,
) -> anyhow::Result<Option<OrderDiscount>> {
    let redeemed = promotion(ctx)?
        .redeem_code(code, order_id, &totals.subtotal, &totals.max_discount())
        .await;
    match redeemed {
        Ok(redeemed) => Ok(Some(OrderDiscount {
            code: redeemed.code,
            kind: match redeemed.kind {
                CodeKind::Discount => PromoKind::Discount,
                CodeKind::Voucher => PromoKind::Voucher,
            },
            amount: redeemed.amount,
        })),
        Err(
            err @ (PromotionError::UnknownCode
            | PromotionError::Inactive
            | PromotionError::Expired
            | PromotionError::LimitReached
            | PromotionError::Cancelled
            | PromotionError::InsufficientBalance { .. }
            | PromotionError::NotApplicable
            | PromotionError::Money(_)),
        ) => {
            tracing::warn!(%order_id, %code, error = %err, "promo code not honoured");
            Ok(None)
        }
        Err(err) => Err(err.into()),
    }
}

/// `OrderCancelled` → the redemption slot is freed, or the voucher gets its
/// money back. Idempotent: a code already given back is a no-op.
#[evento::subscription]
async fn release_code_on_order_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    let order_id = event.aggregate_id.to_owned();
    let Some(order) = load_order_details(ctx.executor, &order_id).await? else {
        anyhow::bail!("order {order_id} cancelled but cannot be loaded");
    };
    let Some(discount) = order.discount else {
        return Ok(());
    };
    promotion(ctx)?
        .release_code(&discount.code, &order_id)
        .await?;
    tracing::info!(%order_id, code = %discount.code, "promo code released");
    Ok(())
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
