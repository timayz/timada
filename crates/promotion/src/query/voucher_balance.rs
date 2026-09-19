//! "Mes bons d'achat et avoirs": a voucher's balance and where it was spent.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::{
    aggregator::{
        Voucher, VoucherCancelled, VoucherIssued, VoucherRedeemed, VoucherRedemptionRefunded,
    },
    value_object::{VoucherKind, VoucherRedemption},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct VoucherView {
    pub id: String,
    pub code: String,
    pub customer_id: Option<String>,
    pub value: Money,
    pub remaining: Money,
    pub kind: VoucherKind,
    pub expires_at: Option<u64>,
    pub cancelled: bool,
    pub cancelled_reason: Option<String>,
    pub redemptions: Vec<VoucherRedemption>,
}

pub fn create_projection<E: Executor>() -> Projection<E, VoucherView> {
    Projection::new::<Voucher>()
        .handler(on_voucher_issued())
        .handler(on_voucher_redeemed())
        .handler(on_voucher_cancelled())
        .handler(on_voucher_redemption_refunded())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<VoucherView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_voucher_issued(
    event: Event<VoucherIssued>,
    row: &mut VoucherView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.code = event.data.code;
    row.customer_id = event.data.customer_id;
    row.remaining = event.data.value.clone();
    row.value = event.data.value;
    row.kind = event.data.kind;
    row.expires_at = event.data.expires_at;
    Ok(())
}

#[evento::handler]
async fn on_voucher_redeemed(
    event: Event<VoucherRedeemed>,
    row: &mut VoucherView,
) -> anyhow::Result<()> {
    row.remaining = row.remaining.checked_sub(&event.data.amount)?;
    row.redemptions.push(VoucherRedemption {
        order_id: event.data.order_id,
        amount: event.data.amount,
    });
    Ok(())
}

#[evento::handler]
async fn on_voucher_cancelled(
    event: Event<VoucherCancelled>,
    row: &mut VoucherView,
) -> anyhow::Result<()> {
    row.cancelled = true;
    row.cancelled_reason = Some(event.data.reason);
    Ok(())
}

#[evento::handler]
async fn on_voucher_redemption_refunded(
    event: Event<VoucherRedemptionRefunded>,
    row: &mut VoucherView,
) -> anyhow::Result<()> {
    row.remaining = row.remaining.checked_add(&event.data.amount)?;
    row.redemptions
        .retain(|r| r.order_id != event.data.order_id);
    Ok(())
}
