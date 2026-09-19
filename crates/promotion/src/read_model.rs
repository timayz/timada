//! SQL list read model behind the admin's promotions section: one row per
//! promo code or voucher. Fed by the `promotion-code-list` subscription; the
//! detail of a code is served by [`crate::DiscountView`] / [`crate::VoucherView`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        DiscountCreated, DiscountDeactivated, DiscountRedeemed, DiscountRedemptionReleased,
        VoucherCancelled, VoucherIssued, VoucherRedeemed, VoucherRedemptionRefunded,
    },
    value_object::DiscountKind,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const PROMOTION_CODE_LIST_SUBSCRIPTION: &str = "promotion-code-list";

pub const DISCOUNT: &str = "discount";
pub const VOUCHER: &str = "voucher";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CodeListRow {
    /// Aggregate id of the `Discount` or `Voucher`.
    pub id: String,
    pub code: String,
    /// [`DISCOUNT`] or [`VOUCHER`].
    pub kind: String,
    /// Percentage in basis points, for a percent promo code.
    pub percent_bp: Option<i64>,
    /// Fixed amount of a promo code, or the value a voucher was issued with.
    pub amount_minor: Option<i64>,
    pub currency: Option<String>,
    /// `false` once deactivated (promo code) or cancelled (voucher).
    pub active: bool,
    /// Unix seconds of creation.
    pub created_at: i64,
}

/// Filters for [`list_codes`]; `kind` is [`DISCOUNT`] or [`VOUCHER`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListCodes {
    pub kind: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListCodes {
    fn default() -> Self {
        Self {
            kind: None,
            limit: 50,
            offset: 0,
        }
    }
}

pub fn code_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PROMOTION_CODE_LIST_SUBSCRIPTION)
        .handler(insert_on_discount_created())
        .handler(deactivate_on_discount_deactivated())
        .skip::<DiscountRedeemed>()
        .skip::<DiscountRedemptionReleased>()
        .handler(insert_on_voucher_issued())
        .handler(deactivate_on_voucher_cancelled())
        .skip::<VoucherRedeemed>()
        .skip::<VoucherRedemptionRefunded>()
        .strict()
}

/// Codes matching the filter, newest first.
pub async fn list_codes(db: &SqlitePool, filter: &ListCodes) -> sqlx::Result<Vec<CodeListRow>> {
    sqlx::query_as(
        "SELECT id, code, kind, percent_bp, amount_minor, currency, active, created_at
         FROM promotion_code_list
         WHERE (?1 IS NULL OR kind = ?1)
         ORDER BY created_at DESC, code
         LIMIT ?2 OFFSET ?3",
    )
    .bind(filter.kind.as_deref())
    .bind(filter.limit)
    .bind(filter.offset)
    .fetch_all(db)
    .await
}

pub async fn count_codes(db: &SqlitePool, kind: Option<&str>) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM promotion_code_list WHERE (?1 IS NULL OR kind = ?1)")
        .bind(kind)
        .fetch_one(db)
        .await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

async fn deactivate<E: Executor>(ctx: &Context<'_, E>, id: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE promotion_code_list SET active = 0 WHERE id = ?")
        .bind(id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn insert_on_discount_created<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<DiscountCreated>,
) -> anyhow::Result<()> {
    let (percent_bp, amount_minor, currency) = match &event.data.kind {
        DiscountKind::Percent { bp } => (Some(i64::from(*bp)), None, None),
        DiscountKind::FixedAmount { amount } => {
            (None, Some(amount.minor), Some(amount.currency.clone()))
        }
    };
    sqlx::query(
        "INSERT OR IGNORE INTO promotion_code_list
            (id, code, kind, percent_bp, amount_minor, currency, active, created_at)
         VALUES (?, ?, ?, ?, ?, ?, 1, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.code)
    .bind(DISCOUNT)
    .bind(percent_bp)
    .bind(amount_minor)
    .bind(currency)
    .bind(event.timestamp as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn deactivate_on_discount_deactivated<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<DiscountDeactivated>,
) -> anyhow::Result<()> {
    deactivate(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn insert_on_voucher_issued<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<VoucherIssued>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO promotion_code_list
            (id, code, kind, percent_bp, amount_minor, currency, active, created_at)
         VALUES (?, ?, ?, NULL, ?, ?, 1, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.code)
    .bind(VOUCHER)
    .bind(event.data.value.minor)
    .bind(&event.data.value.currency)
    .bind(event.timestamp as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn deactivate_on_voucher_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<VoucherCancelled>,
) -> anyhow::Result<()> {
    deactivate(ctx, &event.aggregate_id).await
}
