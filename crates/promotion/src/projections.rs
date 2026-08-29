//! The `admin_discount_list` read model.

use evento::metadata::Event;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::{Currency, Executor, Money};

use crate::aggregate::{DiscountCreated, DiscountDisabled, DiscountKind};
use crate::state::PromotionState;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const READ_MODELS_SUBSCRIPTION: &str = "promotion-read-models";

/// One row of the admin discount list, with its live redemption count joined
/// from the counter table.
#[derive(Debug, sqlx::FromRow)]
pub struct AdminDiscountRow {
    pub id: String,
    pub code: String,
    /// `"percentage"` or `"fixed"`.
    pub kind: String,
    /// Basis points for a percentage, cents for a fixed amount.
    pub value: i64,
    /// Only for fixed amounts.
    pub currency: Option<String>,
    pub starts_at: i64,
    pub ends_at: Option<i64>,
    pub usage_limit: Option<i64>,
    /// `"active"` or `"disabled"`.
    pub status: String,
    pub redeemed: i64,
    pub created_at: i64,
}

impl AdminDiscountRow {
    /// Human description of what the code takes off.
    pub fn describe(&self) -> String {
        if self.kind == "percentage" {
            let bps = self.value;
            format!("{}.{:02} %", bps / 100, bps % 100)
        } else {
            let currency = self
                .currency
                .as_deref()
                .and_then(|code| Currency::from_code(code).ok())
                .unwrap_or_default();
            Money::new(self.value, currency).to_string()
        }
    }

    pub fn window(&self) -> String {
        let from = timada_core::format_utc_date(self.starts_at);
        match self.ends_at {
            Some(ends_at) => format!("{from} → {}", timada_core::format_utc_date(ends_at)),
            None => format!("{from} → open-ended"),
        }
    }

    pub fn usage(&self) -> String {
        match self.usage_limit {
            Some(limit) => format!("{} / {limit}", self.redeemed),
            None => format!("{} / ∞", self.redeemed),
        }
    }
}

/// Newest discounts first.
pub async fn recent_discounts(
    read_pool: &SqlitePool,
    limit: i64,
) -> anyhow::Result<Vec<AdminDiscountRow>> {
    let rows = sqlx::query_as::<_, AdminDiscountRow>(
        "SELECT d.id, d.code, d.kind, d.value, d.currency, d.starts_at, d.ends_at,
                d.usage_limit, d.status, d.created_at,
                COALESCE(r.redeemed, 0) AS redeemed
           FROM admin_discount_list d
           LEFT JOIN discount_redemptions r ON r.discount_id = d.id
          ORDER BY d.created_at DESC, d.id DESC
          LIMIT ?",
    )
    .bind(limit)
    .fetch_all(read_pool)
    .await?;

    Ok(rows)
}

/// The read-model subscription, unstarted — tests drive it with
/// `.no_retry().run_once(&executor)`.
pub fn read_models_subscription(write_pool: SqlitePool) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(READ_MODELS_SUBSCRIPTION)
        .data(write_pool)
        .handler(on_discount_created())
        .handler(on_discount_disabled())
        .strict()
}

/// Spawn the read-model subscription. The caller keeps the handle and calls
/// `shutdown()` on it.
pub async fn start_subscriptions(state: &PromotionState) -> anyhow::Result<Vec<Subscription>> {
    let read_models = read_models_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    tracing::info!("promotion subscriptions started");
    Ok(vec![read_models])
}

fn write_pool<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>().ok_or_else(|| {
        anyhow::anyhow!(
            "`{READ_MODELS_SUBSCRIPTION}` subscription was started without a write pool"
        )
    })
}

#[evento::subscription]
async fn on_discount_created<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<DiscountCreated>,
) -> anyhow::Result<()> {
    let created_at = i64::try_from(event.timestamp)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));

    let (kind, value, currency) = match event.data.kind {
        DiscountKind::Percentage { bps } => ("percentage", i64::from(bps), None),
        DiscountKind::Fixed { amount } => {
            ("fixed", amount.amount_cents, Some(amount.currency.code()))
        }
    };

    sqlx::query(
        "INSERT INTO admin_discount_list
             (id, code, kind, value, currency, starts_at, ends_at, usage_limit,
              status, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'active', ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.code)
    .bind(kind)
    .bind(value)
    .bind(currency)
    .bind(event.data.starts_at)
    .bind(event.data.ends_at)
    .bind(event.data.usage_limit.map(i64::from))
    .bind(created_at)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_discount_disabled<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<DiscountDisabled>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE admin_discount_list SET status = 'disabled' WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&write_pool(ctx)?)
        .await?;

    Ok(())
}
