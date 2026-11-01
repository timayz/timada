//! The subscription that keeps `discounts_codes` current for every tenant.
//!
//! The handler reloads the strict write model of the discount and rewrites
//! its row, so delivery may repeat (the subscription is at-least-once)
//! without the row drifting. One subscription follows every tenant's
//! streams: each row carries the tenant of the event that made it, and the
//! rows live in the store's own database ([`Db`]).

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::Db;

use crate::aggregator::Discount;
use crate::command::{DiscountState, discount_projection};
use crate::value_object::DiscountValue;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-discounts/read-models";

/// Builds the subscription over every tenant and routing key. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_discount_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_discount_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Discount>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(discount) = discount_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_discount(&db, &event.tenant, &discount).await
}

/// Writes the row of a discount as the write model sees it.
pub async fn upsert_discount(
    db: &Db,
    tenant_id: &str,
    discount: &DiscountState,
) -> anyhow::Result<()> {
    let (percent_bps, amount_cents, currency) = match &discount.value {
        DiscountValue::Percent(bps) => (Some(i64::from(*bps)), None, None),
        DiscountValue::Amount { cents, currency } => {
            (None, Some(*cents as i64), Some(currency.as_str()))
        }
    };
    db.execute(
        "INSERT INTO discounts_codes (tenant_id, id, code, percent_bps, amount_cents, currency,
            starts_at, ends_at, max_uses, once_per_email, status, created_by, created_at,
            updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, updated_at = excluded.updated_at",
        (
            tenant_id,
            discount.id.as_str(),
            discount.code.as_str(),
            percent_bps,
            amount_cents,
            currency,
            discount.starts_at.map(|at| at as i64),
            discount.ends_at.map(|at| at as i64),
            discount.max_uses.map(i64::from),
            discount.once_per_email,
            discount.status.as_str(),
            discount.created_by.as_str(),
            discount.created_at as i64,
            discount.updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}
