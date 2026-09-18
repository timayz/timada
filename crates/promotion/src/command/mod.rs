mod cancel_voucher;
mod create_discount;
mod deactivate_discount;
mod issue_voucher;
mod redeem_discount;
mod redeem_voucher;

pub use create_discount::CreateDiscount;
pub use issue_voucher::IssueVoucher;

use evento::{Executor, Projection, metadata::Event};
use sqlx::SqlitePool;
use timada_core::Money;

use crate::{
    aggregator::{
        Discount, DiscountCreated, DiscountDeactivated, DiscountRedeemed, Voucher,
        VoucherCancelled, VoucherIssued, VoucherRedeemed,
    },
    value_object::normalize_code,
};

/// Deterministic discount id: one promo code per (normalised) code.
pub fn discount_id(code: &str) -> String {
    timada_core::id::derived(&[&normalize_code(code)], "discount")
}

/// Deterministic voucher id: one voucher per (normalised) code.
pub fn voucher_id(code: &str) -> String {
    timada_core::id::derived(&[&normalize_code(code)], "voucher")
}

/// Commands need the event store and the SQL pool holding the redemption
/// counter, which must never be derived from event counts under contention.
pub struct Command<'a, E: Executor> {
    pub executor: &'a E,
    pub db: SqlitePool,
}

impl<E: Executor> Command<'_, E> {
    pub async fn load_discount(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<DiscountState>> {
        discount_projection().load(id).execute(self.executor).await
    }

    pub async fn load_voucher(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<VoucherState>> {
        voucher_projection().load(id).execute(self.executor).await
    }
}

/// Write-side state for a promo code.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct DiscountState {
    pub id: String,
    pub code: String,
    pub active: bool,
    pub max_redemptions: Option<u32>,
    pub valid_until: Option<u64>,
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn discount_projection<E: Executor>() -> Projection<E, DiscountState> {
    Projection::new::<Discount>()
        .handler(on_discount_created())
        .handler(on_discount_deactivated())
        .skip::<DiscountRedeemed>()
        .strict()
}

#[evento::handler]
async fn on_discount_created(
    event: Event<DiscountCreated>,
    row: &mut DiscountState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.code = event.data.code;
    row.active = true;
    row.max_redemptions = event.data.max_redemptions;
    row.valid_until = event.data.valid_until;
    Ok(())
}

#[evento::handler]
async fn on_discount_deactivated(
    _event: Event<DiscountDeactivated>,
    row: &mut DiscountState,
) -> anyhow::Result<()> {
    row.active = false;
    Ok(())
}

/// Write-side state for a voucher: the remaining balance.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct VoucherState {
    pub id: String,
    pub code: String,
    pub remaining: Money,
    pub cancelled: bool,
    pub expires_at: Option<u64>,
}

fn voucher_projection<E: Executor>() -> Projection<E, VoucherState> {
    Projection::new::<Voucher>()
        .handler(on_voucher_issued())
        .handler(on_voucher_redeemed())
        .handler(on_voucher_cancelled())
        .strict()
}

#[evento::handler]
async fn on_voucher_issued(
    event: Event<VoucherIssued>,
    row: &mut VoucherState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.code = event.data.code;
    row.remaining = event.data.value;
    row.expires_at = event.data.expires_at;
    Ok(())
}

#[evento::handler]
async fn on_voucher_redeemed(
    event: Event<VoucherRedeemed>,
    row: &mut VoucherState,
) -> anyhow::Result<()> {
    row.remaining = row.remaining.checked_sub(&event.data.amount)?;
    Ok(())
}

#[evento::handler]
async fn on_voucher_cancelled(
    _event: Event<VoucherCancelled>,
    row: &mut VoucherState,
) -> anyhow::Result<()> {
    row.cancelled = true;
    Ok(())
}
