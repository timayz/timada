//! The subscription that keeps `orders_orders` and `orders_carts` current
//! for one tenant.
//!
//! Each handler reloads the strict write model of the stream the event
//! belongs to and rewrites its row, so delivery may repeat (the subscription
//! is at-least-once) without the row drifting. The subscription runs on the
//! tenant's own executor (its cursor lives there) and writes into the shared
//! read-model database it is given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::{Cart, Order};
use crate::command::{CartState, OrderState, cart_projection, order_projection};

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-orders/read-models";

/// The tenant whose stream a subscription follows, handed to its handlers.
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

/// Builds the subscription of one tenant over every routing key. `pool` is
/// the shared read-model database and must be allowed to write; start it
/// with `.start(&tenant_executor)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    tenant_id: &str,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(TenantId(tenant_id.to_owned()))
        .handler(on_cart_event())
        .handler(on_order_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_cart_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Cart>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(cart) = cart_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_cart(&pool, &tenant.0, &cart).await
}

#[evento::subscription_all]
async fn on_order_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Order>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(order) = order_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_order(&pool, &tenant.0, &order).await
}

/// Writes the row of a cart as the write model sees it.
pub async fn upsert_cart(
    pool: &SqlitePool,
    tenant_id: &str,
    cart: &CartState,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO orders_carts (tenant_id, id, order_id, line_count, item_count, created_at,
            updated_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            order_id = excluded.order_id, line_count = excluded.line_count,
            status = excluded.status,
            item_count = excluded.item_count, updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&cart.id)
    .bind(&cart.order_id)
    .bind(cart.lines.len() as i64)
    .bind(i64::from(cart.item_count()))
    .bind(cart.created_at as i64)
    .bind(cart.updated_at as i64)
    .bind(cart.status())
    .execute(pool)
    .await?;
    Ok(())
}

/// Writes the row of an order as the write model sees it.
pub async fn upsert_order(
    pool: &SqlitePool,
    tenant_id: &str,
    order: &OrderState,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO orders_orders (tenant_id, id, reference, cart_id, status, email, name,
            country, item_count, total_cents, currency, placed_at, updated_at, payment_id,
            refunded_cents, discount_id, discount_code, discount_cents, fulfilled_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
            ?18, ?19)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, updated_at = excluded.updated_at,
            refunded_cents = excluded.refunded_cents, discount_id = excluded.discount_id,
            discount_code = excluded.discount_code, discount_cents = excluded.discount_cents,
            fulfilled_at = excluded.fulfilled_at",
    )
    .bind(tenant_id)
    .bind(&order.id)
    .bind(&order.reference)
    .bind(&order.cart_id)
    .bind(order.status.as_str())
    .bind(&order.email)
    .bind(&order.shipping.name)
    .bind(&order.shipping.country)
    .bind(i64::from(order.item_count()))
    .bind(order.total_cents as i64)
    .bind(&order.currency)
    .bind(order.placed_at as i64)
    .bind(order.updated_at as i64)
    .bind(&order.payment.payment_id)
    .bind(order.refunded_cents as i64)
    .bind(order.discount.as_ref().map(|d| d.discount_id.as_str()))
    .bind(order.discount.as_ref().map(|d| d.code.as_str()))
    .bind(order.discount.as_ref().map_or(0, |d| d.amount_cents as i64))
    .bind(order.fulfilled_at.map(|at| at as i64))
    .execute(pool)
    .await?;
    Ok(())
}
