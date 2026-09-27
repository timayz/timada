//! The purchases, as the back office lists them: « À commander » first, then
//! what has been bought and what became of it. Fed by the
//! `sourcing-purchases` subscription, each handler rewriting a whole row from
//! the view so a redelivered event changes nothing.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_core::Money;

use crate::{
    aggregator::{
        SupplierOrderCancelled, SupplierOrderDrafted, SupplierOrderPlaced,
        SupplierOrderRecordedByHand, SupplierOrderRefused, SupplierOrderShipped,
    },
    query::load_supplier_order,
    value_object::SupplierOrderStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const PURCHASE_LIST_SUBSCRIPTION: &str = "sourcing-purchases";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseRow {
    pub purchase_id: String,
    pub order_id: String,
    pub supplier_id: String,
    pub status: SupplierOrderStatus,
    pub external_order_id: Option<String>,
    /// What it was expected to come to.
    pub cost: Money,
    /// What the supplier charged, once it took it.
    pub charged: Option<Money>,
    pub units: i64,
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    pub note: Option<String>,
    pub drafted_at: i64,
    pub settled_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListPurchases {
    /// `None` keeps them all; the queue shows `Drafted`.
    pub status: Option<SupplierOrderStatus>,
    pub supplier_id: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl ListPurchases {
    pub fn to_order(limit: u32, offset: u32) -> Self {
        Self {
            status: Some(SupplierOrderStatus::Drafted),
            limit,
            offset,
            ..Self::default()
        }
    }
}

pub fn purchase_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PURCHASE_LIST_SUBSCRIPTION)
        .handler(refresh_on_drafted())
        .handler(refresh_on_placed())
        .handler(refresh_on_by_hand())
        .handler(refresh_on_refused())
        .handler(refresh_on_shipped())
        .handler(refresh_on_cancelled())
        .strict()
}

pub async fn list_purchases(
    db: &SqlitePool,
    filter: &ListPurchases,
) -> sqlx::Result<Vec<PurchaseRow>> {
    let rows: Vec<RawPurchase> = sqlx::query_as(
        "SELECT purchase_id, order_id, supplier_id, status, external_order_id,
                cost_minor, cost_currency, charged_minor, units, carrier, tracking_number,
                note, drafted_at, settled_at
         FROM sourcing_purchase
         WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR supplier_id = ?2)
         ORDER BY settled_at IS NOT NULL, drafted_at DESC, purchase_id
         LIMIT ?3 OFFSET ?4",
    )
    .bind(filter.status.map(|status| status.as_str()))
    .bind(filter.supplier_id.as_deref())
    .bind(filter.limit.max(1))
    .bind(filter.offset)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn count_purchases(db: &SqlitePool, filter: &ListPurchases) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM sourcing_purchase
         WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR supplier_id = ?2)",
    )
    .bind(filter.status.map(|status| status.as_str()))
    .bind(filter.supplier_id.as_deref())
    .fetch_one(db)
    .await
}

/// What is being bought for one order — what the order page shows, and what
/// customer service answers « où est ma commande ? » with.
pub async fn purchases_for_order(
    db: &SqlitePool,
    order_id: &str,
) -> sqlx::Result<Vec<PurchaseRow>> {
    let rows: Vec<RawPurchase> = sqlx::query_as(
        "SELECT purchase_id, order_id, supplier_id, status, external_order_id,
                cost_minor, cost_currency, charged_minor, units, carrier, tracking_number,
                note, drafted_at, settled_at
         FROM sourcing_purchase WHERE order_id = ? ORDER BY drafted_at, purchase_id",
    )
    .bind(order_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn purchase_by_id(db: &SqlitePool, id: &str) -> sqlx::Result<Option<PurchaseRow>> {
    let row: Option<RawPurchase> = sqlx::query_as(
        "SELECT purchase_id, order_id, supplier_id, status, external_order_id,
                cost_minor, cost_currency, charged_minor, units, carrier, tracking_number,
                note, drafted_at, settled_at
         FROM sourcing_purchase WHERE purchase_id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(Into::into))
}

#[derive(sqlx::FromRow)]
struct RawPurchase {
    purchase_id: String,
    order_id: String,
    supplier_id: String,
    status: String,
    external_order_id: Option<String>,
    cost_minor: i64,
    cost_currency: String,
    charged_minor: Option<i64>,
    units: i64,
    carrier: Option<String>,
    tracking_number: Option<String>,
    note: Option<String>,
    drafted_at: i64,
    settled_at: Option<i64>,
}

impl From<RawPurchase> for PurchaseRow {
    fn from(row: RawPurchase) -> Self {
        Self {
            status: SupplierOrderStatus::parse(&row.status).unwrap_or_default(),
            charged: row
                .charged_minor
                .map(|minor| Money::new(minor, row.cost_currency.clone())),
            cost: Money::new(row.cost_minor, row.cost_currency),
            purchase_id: row.purchase_id,
            order_id: row.order_id,
            supplier_id: row.supplier_id,
            external_order_id: row.external_order_id,
            units: row.units,
            carrier: row.carrier,
            tracking_number: row.tracking_number,
            note: row.note,
            drafted_at: row.drafted_at,
            settled_at: row.settled_at,
        }
    }
}

async fn refresh<E: Executor>(ctx: &Context<'_, E>, purchase_id: &str) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(purchase) = load_supplier_order(ctx.executor, purchase_id).await? else {
        anyhow::bail!("purchase {purchase_id} cannot be loaded");
    };
    sqlx::query(
        "INSERT INTO sourcing_purchase
            (purchase_id, order_id, supplier_id, status, external_order_id,
             cost_minor, cost_currency, charged_minor, units, carrier, tracking_number,
             note, drafted_at, settled_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (purchase_id) DO UPDATE SET
            status = excluded.status,
            external_order_id = excluded.external_order_id,
            cost_minor = excluded.cost_minor,
            cost_currency = excluded.cost_currency,
            charged_minor = excluded.charged_minor,
            units = excluded.units,
            carrier = excluded.carrier,
            tracking_number = excluded.tracking_number,
            note = excluded.note,
            settled_at = excluded.settled_at",
    )
    .bind(&purchase.id)
    .bind(&purchase.order_id)
    .bind(&purchase.supplier_id)
    .bind(purchase.status.as_str())
    .bind(purchase.external_order_id.as_deref())
    .bind(purchase.cost.minor)
    .bind(&purchase.cost.currency)
    .bind(purchase.charged.as_ref().map(|money| money.minor))
    .bind(i64::from(purchase.units()))
    .bind(purchase.carrier.as_deref())
    .bind(purchase.tracking_number.as_deref())
    .bind(purchase.note.as_deref())
    .bind(purchase.drafted_at as i64)
    .bind(purchase.settled_at.map(|at| at as i64))
    .execute(&db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_drafted<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderDrafted>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_placed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderPlaced>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_by_hand<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderRecordedByHand>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_refused<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderRefused>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_shipped<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderShipped>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderCancelled>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}
