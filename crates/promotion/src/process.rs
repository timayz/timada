//! Anti-corruption layer from the order context: an `OrderPlaced` carrying a
//! promo code redeems it. Validation failures are logged, not retried — the
//! order stands; enforcing code validity at checkout is a follow-up. Not
//! strict (one event of a foreign aggregate) and `continue_on_error` so a
//! transient failure on one order never stalls the others.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_order::aggregator::OrderPlaced;

use crate::{command::Command, error::PromotionError};

pub const REDEEM_ON_ORDER_SUBSCRIPTION: &str = "promotion-redeem-on-order";

pub fn redeem_on_order_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(REDEEM_ON_ORDER_SUBSCRIPTION)
        .handler(redeem_on_order_placed())
        .continue_on_error()
}

#[evento::subscription]
async fn redeem_on_order_placed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPlaced>,
) -> anyhow::Result<()> {
    let Some(code) = event.data.promo_code.as_deref() else {
        return Ok(());
    };
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let order_id = event.aggregate_id.to_owned();

    let command = Command {
        executor: ctx.executor,
        db,
    };
    match command.redeem_discount(code, &order_id).await {
        Ok(()) => Ok(()),
        Err(
            err @ (PromotionError::UnknownCode
            | PromotionError::Inactive
            | PromotionError::Expired
            | PromotionError::LimitReached),
        ) => {
            tracing::warn!(%order_id, %code, error = %err, "promo code not redeemed");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
