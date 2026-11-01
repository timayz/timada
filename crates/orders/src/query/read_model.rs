//! The subscription that keeps `orders_orders` and `orders_carts` current
//! for every tenant.
//!
//! Each handler reloads the strict write model of the stream the event
//! belongs to and rewrites its row, so delivery may repeat (the subscription
//! is at-least-once) without the row drifting. One subscription follows
//! every tenant's streams: each row carries the tenant of the event that made
//! it, and it writes into the read-model database it is given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, Value, params_from_iter};

use crate::aggregator::{Cart, Order};
use crate::command::{CartState, OrderState, cart_projection, order_projection};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-orders/read-models";

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write; start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_cart_event())
        .handler(on_order_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_cart_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Cart>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(cart) = cart_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_cart(&db, &event.tenant, &cart).await
}

#[evento::subscription_all]
async fn on_order_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Order>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(order) = order_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_order(&db, &event.tenant, &order).await
}

/// Writes the row of a cart as the write model sees it.
pub async fn upsert_cart(db: &Db, tenant_id: &str, cart: &CartState) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO orders_carts (tenant_id, id, order_id, line_count, item_count, created_at,
            updated_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            order_id = excluded.order_id, line_count = excluded.line_count,
            status = excluded.status,
            item_count = excluded.item_count, updated_at = excluded.updated_at",
        (
            tenant_id,
            cart.id.as_str(),
            cart.order_id.as_deref(),
            cart.lines.len() as i64,
            i64::from(cart.item_count()),
            cart.created_at as i64,
            cart.updated_at as i64,
            cart.status(),
        ),
    )
    .await?;
    Ok(())
}

/// Writes the row of an order as the write model sees it.
pub async fn upsert_order(db: &Db, tenant_id: &str, order: &OrderState) -> anyhow::Result<()> {
    db.execute(
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
        params_from_iter([
            Value::from(tenant_id),
            Value::from(order.id.as_str()),
            Value::from(order.reference.as_str()),
            Value::from(order.cart_id.as_str()),
            Value::from(order.status.as_str()),
            Value::from(order.email.as_str()),
            Value::from(order.shipping.name.as_str()),
            Value::from(order.shipping.country.as_str()),
            Value::from(i64::from(order.item_count())),
            Value::from(order.total_cents as i64),
            Value::from(order.currency.as_str()),
            Value::from(order.placed_at as i64),
            Value::from(order.updated_at as i64),
            Value::from(order.payment.payment_id.as_str()),
            Value::from(order.refunded_cents as i64),
            Value::from(order.discount.as_ref().map(|d| d.discount_id.as_str())),
            Value::from(order.discount.as_ref().map(|d| d.code.as_str())),
            Value::from(order.discount.as_ref().map_or(0, |d| d.amount_cents as i64)),
            Value::from(order.fulfilled_at.map(|at| at as i64)),
        ]),
    )
    .await?;
    Ok(())
}
