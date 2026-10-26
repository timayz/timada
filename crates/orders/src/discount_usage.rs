//! Spends the use of a discount code once the order it priced is paid.
//!
//! The dashboard reserves the use before it asks the shopper to pay; this
//! subscription turns the reservation into a spent use when the order is
//! placed, so the reservation never lapses and gives back a use an order
//! already took. Delivery may repeat: spending a use is idempotent.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};

use crate::aggregator::Discounted;
use crate::command::order_projection;

/// The cursor identity of the subscription, in the store.
pub const DISCOUNT_USAGE_SUBSCRIPTION: &str = "timada-orders/discount-usage";

/// Who spends the uses, in the discount streams.
const ORDERS_ACTOR: &str = "timada-orders";

/// The store, to write with (a handler only borrows the executor it runs
/// on).
#[derive(Clone)]
struct Store(Evento);

/// Builds the subscription over every tenant and routing key; `store` is
/// the store it follows, which it writes the spent uses to (in the tenant
/// of the order).
pub fn discount_usage_subscription<E: Executor>(store: Evento) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(DISCOUNT_USAGE_SUBSCRIPTION)
        .data(Store(store))
        .handler(on_discounted())
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_discounted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Discounted>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    let Some(order) = order_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let discounts = timada_discounts::Command::new(store.0, &event.tenant);
    match discounts
        .consume_use(
            ORDERS_ACTOR,
            &event.data.discount_id,
            &order.cart_id,
            &order.email,
            &order.id,
        )
        .await
    {
        Ok(()) => Ok(()),
        // The discount is gone from the store: nothing left to count.
        Err(timada_discounts::DiscountError::NotFound) => Ok(()),
        Err(err) => Err(err.into()),
    }
}
