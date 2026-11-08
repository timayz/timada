//! The subscription that keeps `orders_orders`, `orders_carts`,
//! `orders_order_products` (which products each order holds, for whoever
//! asks whether a product was ever ordered) and `orders_order_mails` (which
//! mails the shopper was sent about an order) current for every tenant.
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
use timada_core::db::{Db, Value, execute, params_from_iter};

use crate::aggregator::{Cart, Order, OrderMail};
use crate::command::{
    CartState, OrderState, cart_projection, order_mail_projection, order_projection,
};
use crate::query::OrderRecord;

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
        .handler(on_order_mail_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_order_mail_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<OrderMail>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(mailed) = order_mail_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    // A mail is sent once; a repeat finds its row there.
    db.execute(
        "INSERT OR IGNORE INTO orders_order_mails (tenant_id, order_id, mail, mailed_at)
         VALUES (?1, ?2, ?3, ?4)",
        (
            event.tenant.as_str(),
            mailed.order_id.as_str(),
            mailed.mail.as_str(),
            mailed.mailed_at as i64,
        ),
    )
    .await?;
    Ok(())
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
pub(crate) async fn upsert_cart(db: &Db, tenant_id: &str, cart: &CartState) -> anyhow::Result<()> {
    let refused_returns = serde_json::to_string(&cart.refused_returns())?;
    db.execute(
        "INSERT INTO orders_carts (tenant_id, id, order_id, line_count, item_count, created_at,
            updated_at, status, return_requested_at, unanswered_since, refused_returns)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            order_id = excluded.order_id, line_count = excluded.line_count,
            status = excluded.status,
            item_count = excluded.item_count, updated_at = excluded.updated_at,
            return_requested_at = excluded.return_requested_at,
            unanswered_since = excluded.unanswered_since,
            refused_returns = excluded.refused_returns",
        (
            tenant_id,
            cart.id.as_str(),
            cart.order_id.as_deref(),
            cart.lines.len() as i64,
            i64::from(cart.item_count()),
            cart.created_at as i64,
            cart.updated_at as i64,
            cart.status(),
            cart.return_requested_at().map(|at| at as i64),
            cart.unanswered_since().map(|at| at as i64),
            refused_returns.as_str(),
        ),
    )
    .await?;
    Ok(())
}

/// Writes the row of an order as the write model sees it (the whole order
/// as its page shows it in `state`), and one row per product it holds, in
/// one transaction.
pub(crate) async fn upsert_order(
    db: &Db,
    tenant_id: &str,
    order: &OrderState,
) -> anyhow::Result<()> {
    let state = OrderRecord::of(order).to_json()?;
    let env = (tenant_id, order, state);
    db.write(&env, |tx, (tenant_id, order, state)| {
        Box::pin(async move {
            execute(
                tx,
                "INSERT INTO orders_orders (tenant_id, id, reference, cart_id, status, email, name,
                    country, item_count, total_cents, currency, placed_at, updated_at, payment_id,
                    refunded_cents, discount_id, discount_code, discount_cents, fulfilled_at,
                    supplier, payment_state, capture_before, capture_requested_at,
                    cancel_requested_at, paid_at, refund_requested_at, hold_reminder_at, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                    ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET
                    status = excluded.status, updated_at = excluded.updated_at,
                    refunded_cents = excluded.refunded_cents, discount_id = excluded.discount_id,
                    discount_code = excluded.discount_code, discount_cents = excluded.discount_cents,
                    fulfilled_at = excluded.fulfilled_at, supplier = excluded.supplier,
                    payment_state = excluded.payment_state,
                    capture_before = excluded.capture_before,
                    capture_requested_at = excluded.capture_requested_at,
                    cancel_requested_at = excluded.cancel_requested_at,
                    paid_at = excluded.paid_at,
                    refund_requested_at = excluded.refund_requested_at,
                    hold_reminder_at = excluded.hold_reminder_at, state = excluded.state",
                params_from_iter([
                    Value::from(*tenant_id),
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
                    Value::from(order.supplier.as_deref()),
                    Value::from(order.payment_state()),
                    Value::from(order.payment.capture_before.map(|at| at as i64)),
                    Value::from(
                        order
                            .pending_capture
                            .as_ref()
                            .map(|pending| pending.requested_at as i64),
                    ),
                    Value::from(
                        order
                            .pending_cancellation
                            .as_ref()
                            .map(|pending| pending.requested_at as i64),
                    ),
                    Value::from(order.paid_at.map(|at| at as i64)),
                    Value::from(
                        order
                            .pending_refund
                            .as_ref()
                            .map(|pending| pending.requested_at as i64),
                    ),
                    Value::from(order.hold_reminder_due_at().map(|at| at as i64)),
                    Value::from(state.as_str()),
                ]),
            )
            .await?;
            // The lines of an order never change once placed; a repeat finds
            // its rows there.
            for line in &order.lines {
                execute(
                    tx,
                    "INSERT OR IGNORE INTO orders_order_products (tenant_id, product_id, order_id)
                     VALUES (?1, ?2, ?3)",
                    (*tenant_id, line.product_id.as_str(), order.id.as_str()),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}
