//! Admin/checkout view of a promo code. Executor-backed snapshots via bitcode.

use evento::{Executor, metadata::Event, projection::Projection};

use crate::{
    aggregator::{
        Discount, DiscountCreated, DiscountDeactivated, DiscountRedeemed,
        DiscountRedemptionReleased,
    },
    value_object::DiscountKind,
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct DiscountView {
    pub id: String,
    pub code: String,
    pub kind: DiscountKind,
    pub max_redemptions: Option<u32>,
    pub redeemed: u32,
    pub active: bool,
    pub valid_until: Option<u64>,
}

pub fn create_projection<E: Executor>() -> Projection<E, DiscountView> {
    Projection::new::<Discount>()
        .handler(on_discount_created())
        .handler(on_discount_redeemed())
        .handler(on_discount_deactivated())
        .handler(on_discount_redemption_released())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<DiscountView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_discount_created(
    event: Event<DiscountCreated>,
    row: &mut DiscountView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.code = event.data.code;
    row.kind = event.data.kind;
    row.max_redemptions = event.data.max_redemptions;
    row.valid_until = event.data.valid_until;
    row.active = true;
    Ok(())
}

#[evento::handler]
async fn on_discount_redeemed(
    _event: Event<DiscountRedeemed>,
    row: &mut DiscountView,
) -> anyhow::Result<()> {
    row.redeemed += 1;
    Ok(())
}

#[evento::handler]
async fn on_discount_deactivated(
    _event: Event<DiscountDeactivated>,
    row: &mut DiscountView,
) -> anyhow::Result<()> {
    row.active = false;
    Ok(())
}

#[evento::handler]
async fn on_discount_redemption_released(
    _event: Event<DiscountRedemptionReleased>,
    row: &mut DiscountView,
) -> anyhow::Result<()> {
    row.redeemed = row.redeemed.saturating_sub(1);
    Ok(())
}
