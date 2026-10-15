//! The subscription that keeps `discounts_codes` current for one tenant.
//!
//! The handler reloads the strict write model of the discount and rewrites
//! its row, so delivery may repeat (the subscription is at-least-once)
//! without the row drifting. It runs on the tenant's own executor (its cursor
//! lives there) and writes into the shared read-model database it is given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::Discount;
use crate::command::{DiscountState, discount_projection};
use crate::value_object::DiscountValue;

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-discounts/read-models";

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
        .handler(on_discount_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_discount_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Discount>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(discount) = discount_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_discount(&pool, &tenant.0, &discount).await
}

/// Writes the row of a discount as the write model sees it.
pub async fn upsert_discount(
    pool: &SqlitePool,
    tenant_id: &str,
    discount: &DiscountState,
) -> anyhow::Result<()> {
    let (percent_bps, amount_cents, currency) = match &discount.value {
        DiscountValue::Percent(bps) => (Some(i64::from(*bps)), None, None),
        DiscountValue::Amount { cents, currency } => {
            (None, Some(*cents as i64), Some(currency.as_str()))
        }
    };
    sqlx::query(
        "INSERT INTO discounts_codes (tenant_id, id, code, percent_bps, amount_cents, currency,
            starts_at, ends_at, max_uses, once_per_email, status, created_by, created_at,
            updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&discount.id)
    .bind(&discount.code)
    .bind(percent_bps)
    .bind(amount_cents)
    .bind(currency)
    .bind(discount.starts_at.map(|at| at as i64))
    .bind(discount.ends_at.map(|at| at as i64))
    .bind(discount.max_uses.map(i64::from))
    .bind(discount.once_per_email)
    .bind(discount.status.as_str())
    .bind(&discount.created_by)
    .bind(discount.created_at as i64)
    .bind(discount.updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}
