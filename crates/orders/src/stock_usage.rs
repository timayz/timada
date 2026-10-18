//! Takes what a paid order sold off the shelf, and puts it back when the
//! order is cancelled.
//!
//! The dashboard holds the stock before the shopper is asked to pay; this
//! subscription turns the hold into sold goods when the order is placed, so
//! the hold never lapses and gives back what an order already took. Delivery
//! may repeat: an order takes its share, and gives it back, once.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::{CatalogError, StockLine};

use crate::aggregator::{Cancelled, Placed};
use crate::command::order_projection;
use crate::value_object::OrderLine;

/// The cursor identity of the subscription, in each tenant's store.
pub const STOCK_USAGE_SUBSCRIPTION: &str = "timada-orders/stock-usage";

/// Who takes the goods off the shelf, in the stock streams.
const ORDERS_ACTOR: &str = "timada-orders";

/// The tenant's own executor, to write with (a handler only borrows the one
/// it runs on).
#[derive(Clone)]
struct Store(Evento);

/// Builds the subscription of one tenant over every routing key; `store` is
/// the tenant's executor, which it writes the stock to.
pub fn stock_usage_subscription<E: Executor>(store: Evento) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(STOCK_USAGE_SUBSCRIPTION)
        .data(Store(store))
        .handler(on_placed())
        .handler(on_cancelled())
        .any_routing_key()
        .retry(5)
}

/// What the lines of an order took, thing by thing.
pub fn stock_lines(lines: &[OrderLine]) -> Vec<StockLine> {
    lines
        .iter()
        .map(|line| StockLine {
            product_id: line.product_id.clone(),
            variant_id: line.variant_id.clone(),
            quantity: line.quantity,
        })
        .collect()
}

/// A product gone from the store has no stock left to count.
fn settled(result: Result<(), CatalogError>) -> anyhow::Result<()> {
    match result {
        Ok(()) | Err(CatalogError::NotFound) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[evento::subscription]
async fn on_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Placed>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    let lines = stock_lines(&event.data.lines);
    settled(
        timada_catalog::Command(store.0)
            .consume_stock(
                ORDERS_ACTOR,
                &event.data.cart_id,
                &event.aggregate_id,
                &lines,
            )
            .await,
    )
}

#[evento::subscription]
async fn on_cancelled<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Cancelled>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    let Some(order) = order_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    settled(
        timada_catalog::Command(store.0)
            .restore_stock(ORDERS_ACTOR, &order.id, &stock_lines(&order.lines))
            .await,
    )
}
