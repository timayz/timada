//! Buying what was sold, and hearing back about it.
//!
//! Two things happen here. When an order is paid, the lines the shop does not
//! hold are written down as purchases — one per supplier — and go no further:
//! **placing one spends the shop's money, so it is an operator's click**,
//! unless the host says otherwise with [`PurchaseMode`]. And when a supplier
//! reports a carrier and a tracking number, the shop's own shipment is
//! dispatched, which the existing fulfillment saga turns into `OrderShipped`
//! and the « expédié » e-mail — no new order code, no new mailer code.
//!
//! The connector is never called from a page. A click enqueues work in
//! `sourcing_purchase_work`, and the ticker does the talking.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_order::aggregator::{OrderCancelled, OrderPaid, OrderSettled};

use crate::{
    aggregator::SupplierOrderShipped,
    command::{Command, DraftSupplierOrder},
    connector::{ConnectorError, ConnectorTask, PlaceOrder, SupplierConnectors},
    error::SourcingError,
    sourcing_list::sourced_products_by_ids,
    value_object::{SupplierOrderLine, SupplierOrderStatus},
};

/// Subscription key; the caller attaches the pool with `.data(pool)`, and
/// optionally a [`PurchaseMode`].
pub const SOURCING_ORDER_SUBSCRIPTION: &str = "sourcing-orders";

/// When a purchase is actually placed with the supplier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PurchaseMode {
    /// An operator confirms each one from « À commander ». The default,
    /// because this is the shop's money going out.
    #[default]
    OnConfirmation,
    /// Enqueued as soon as the order is paid. Faster, and nobody looks.
    OnPayment,
    /// The connector is never called: the operator buys on the supplier's own
    /// site and types the reference back in. What a supplier with no API gets
    /// whatever this says.
    ByHand,
}

/// Not strict: it watches a subset of two contexts' events.
pub fn sourcing_order_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SOURCING_ORDER_SUBSCRIPTION)
        .handler(draft_on_order_paid())
        .handler(draft_on_order_settled())
        .handler(cancel_on_order_cancelled())
        .handler(dispatch_on_supplier_order_shipped())
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

#[evento::subscription]
async fn draft_on_order_paid<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPaid>,
) -> anyhow::Result<()> {
    draft(ctx, &event.aggregate_id).await
}

/// A voucher covering the whole total settles an order without a payment, and
/// it still has to be bought from the supplier. Every consumer of `OrderPaid`
/// has to handle this one too.
#[evento::subscription]
async fn draft_on_order_settled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderSettled>,
) -> anyhow::Result<()> {
    draft(ctx, &event.aggregate_id).await
}

/// Writes down what has to be bought, grouped by supplier. Lines the shop
/// holds itself are simply absent: a half-dropship order ships those from the
/// warehouse the ordinary way.
async fn draft<E: Executor>(ctx: &Context<'_, E>, order_id: &str) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let Some(order) = timada_order::load_order_details(ctx.executor, order_id).await? else {
        anyhow::bail!("order {order_id} cannot be loaded");
    };
    let product_ids: Vec<String> = order
        .lines
        .iter()
        .map(|line| line.product_id.clone())
        .collect();
    let sourced = sourced_products_by_ids(&db, &product_ids).await?;
    if sourced.is_empty() {
        return Ok(());
    }

    let cmd = Command::new(ctx.executor, db.clone());
    let mut by_supplier: BTreeMap<String, Vec<SupplierOrderLine>> = BTreeMap::new();
    for line in &order.lines {
        let Some(row) = sourced.iter().find(|row| row.product_id == line.product_id) else {
            continue;
        };
        // What it costs is the supplier's last word; without one the draft
        // still stands, at nothing, so an operator sees it and can act.
        let unit_cost = crate::sourcing_list::offer_of_product(&db, &line.product_id)
            .await?
            .map(|offer| offer.cost)
            .unwrap_or_default();
        by_supplier
            .entry(row.supplier_id.clone())
            .or_default()
            .push(SupplierOrderLine {
                product_id: line.product_id.clone(),
                external_item_id: row.external_item_id.clone(),
                external_sku: row.external_sku.clone(),
                quantity: line.quantity,
                unit_cost,
            });
    }

    let mode = ctx.get::<PurchaseMode>().unwrap_or_default();
    for (supplier_id, lines) in by_supplier {
        let id = cmd
            .draft_supplier_order(DraftSupplierOrder {
                order_id: order_id.to_owned(),
                supplier_id: supplier_id.clone(),
                lines,
                ship_to: order.delivery_address.clone(),
            })
            .await?;
        if mode == PurchaseMode::OnPayment {
            enqueue_place(&db, &id).await?;
        }
    }
    Ok(())
}

/// The customer's order was called off: the purchase goes with it, as far as
/// it can. A parcel already on its way is a return, not a cancellation.
#[evento::subscription]
async fn cancel_on_order_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let cmd = Command::new(ctx.executor, db.clone());
    for purchase in purchases_of_order(&db, &event.aggregate_id).await? {
        let Some(state) = cmd.load_purchase(&purchase).await? else {
            continue;
        };
        if !state.status.is_open() {
            continue;
        }
        // Told to the supplier by the worker, which has the connector; here
        // the shop simply stops meaning to buy it.
        match cmd
            .cancel_supplier_order(&purchase, "commande annulée".to_owned())
            .await
        {
            Ok(()) => {
                if state.external_order_id.is_some() {
                    enqueue(&db, &purchase, WorkKind::Cancel).await?;
                }
            }
            Err(SourcingError::SupplierOrderShipped) => {
                tracing::warn!(purchase_id = %purchase, "parcel already gone: the way back is a return");
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

/// The supplier shipped. When **every** purchase of the order has, the shop's
/// own shipment is dispatched; the fulfillment saga does the rest.
///
/// `timada-shipping` holds one shipment per order, so an order split across
/// two suppliers travels under the first carrier's tracking and the others
/// are shown on their purchases. Multi-parcel is a shipping question, not a
/// dropshipping one.
#[evento::subscription]
async fn dispatch_on_supplier_order_shipped<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderShipped>,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let cmd = Command::new(ctx.executor, db.clone());
    let Some(purchase) = cmd.load_purchase(&event.aggregate_id).await? else {
        return Ok(());
    };
    for other in purchases_of_order(&db, &purchase.order_id).await? {
        let Some(state) = cmd.load_purchase(&other).await? else {
            continue;
        };
        if state.status.is_open() {
            tracing::info!(
                order_id = %purchase.order_id,
                "a supplier shipped; waiting for the others before dispatching"
            );
            return Ok(());
        }
    }

    let shipment = timada_shipping::shipment_id(&purchase.order_id);
    match timada_shipping::Command(ctx.executor)
        .dispatch_shipment(
            &shipment,
            event.data.carrier.clone(),
            event.data.tracking_number.clone(),
        )
        .await
    {
        Ok(()) => Ok(()),
        // Already dispatched, or there is no parcel to dispatch: both are
        // "nothing left to do", and both are what makes redelivery converge.
        Err(timada_shipping::ShippingError::NotCreated)
        | Err(timada_shipping::ShippingError::ShipmentNotFound) => {
            tracing::info!(%shipment, "shipment needed no dispatching");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// The purchases of one customer order.
pub async fn purchases_of_order(db: &SqlitePool, order_id: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT purchase_id FROM sourcing_purchase WHERE order_id = ? ORDER BY purchase_id",
    )
    .bind(order_id)
    .fetch_all(db)
    .await
}

/// What a work row asks the connector to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkKind {
    Place,
    Track,
    Cancel,
}

impl WorkKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Place => "place",
            Self::Track => "track",
            Self::Cancel => "cancel",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "place" => Some(Self::Place),
            "track" => Some(Self::Track),
            "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// The operator's click: enqueues the purchase, and never calls the supplier
/// here. A page that waits on somebody else's API is a page that times out.
pub async fn enqueue_place(db: &SqlitePool, purchase_id: &str) -> sqlx::Result<()> {
    enqueue(db, purchase_id, WorkKind::Place).await
}

async fn enqueue(db: &SqlitePool, purchase_id: &str, kind: WorkKind) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO sourcing_purchase_work (work_id, purchase_id, kind, next_attempt_at)
         VALUES (?1, ?2, ?3, 0)
         ON CONFLICT (work_id) DO UPDATE SET next_attempt_at = 0, done_at = NULL",
    )
    .bind(format!("{purchase_id}:{}", kind.as_str()))
    .bind(purchase_id)
    .bind(kind.as_str())
    .execute(db)
    .await?;
    Ok(())
}

async fn close(db: &SqlitePool, work_id: &str, last_error: Option<&str>) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sourcing_purchase_work
         SET done_at = ?2, last_error = ?3, claimed_by = NULL, claimed_until = NULL
         WHERE work_id = ?1",
    )
    .bind(work_id)
    .bind(timada_core::time::now_unix_secs().unwrap_or_default() as i64)
    .bind(last_error)
    .execute(db)
    .await?;
    Ok(())
}

/// How patiently the suppliers are talked to about orders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchasePolicy {
    pub retry_delays: Vec<Duration>,
    pub lease: Duration,
    pub batch: u32,
    /// How often a placed purchase is asked about until it ships or is
    /// called off.
    pub track_every: Duration,
    /// How much dearer than drafted a supplier may be before the purchase is
    /// flagged. It is still placed: one unit's overrun is small, and a loss
    /// that is visible beats an order stuck in a retry loop.
    pub cost_tolerance_bp: u16,
}

impl Default for PurchasePolicy {
    fn default() -> Self {
        Self {
            retry_delays: vec![
                Duration::from_secs(60),
                Duration::from_secs(5 * 60),
                Duration::from_secs(30 * 60),
                Duration::from_secs(2 * 60 * 60),
            ],
            lease: Duration::from_secs(5 * 60),
            batch: 50,
            track_every: Duration::from_secs(30 * 60),
            cost_tolerance_bp: 1_000,
        }
    }
}

impl PurchasePolicy {
    pub fn without_delays() -> Self {
        Self {
            retry_delays: Vec::new(),
            track_every: Duration::ZERO,
            ..Self::default()
        }
    }

    fn wait_after(&self, attempts: i64) -> Duration {
        let index = usize::try_from(attempts.max(0)).unwrap_or(usize::MAX);
        self.retry_delays
            .get(index)
            .copied()
            .unwrap_or(Duration::from_secs(6 * 60 * 60))
    }
}

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurchasePass {
    pub placed: u32,
    pub refused: u32,
    pub shipped: u32,
    pub cancelled: u32,
    /// Still on its way at the supplier: asked about again later.
    pub pending: u32,
    pub postponed: u32,
    pub held: u32,
    pub failed: u32,
}

#[derive(Debug, sqlx::FromRow)]
struct WorkRow {
    work_id: String,
    purchase_id: String,
    kind: String,
    attempts: i64,
}

pub async fn work_purchases<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    connectors: &SupplierConnectors,
) -> Result<PurchasePass, SourcingError> {
    work_purchases_with(executor, db, connectors, &PurchasePolicy::default()).await
}

/// Places, tracks and calls off what is due, claiming its rows so any number
/// of workers may run.
pub async fn work_purchases_with<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    connectors: &SupplierConnectors,
    policy: &PurchasePolicy,
) -> Result<PurchasePass, SourcingError> {
    let now = timada_core::time::now_unix_secs()? as i64;
    let worker = worker_id();
    let rows: Vec<WorkRow> = sqlx::query_as(
        "UPDATE sourcing_purchase_work
         SET claimed_by = ?1, claimed_until = ?2
         WHERE work_id IN (
            SELECT work_id FROM sourcing_purchase_work
            WHERE done_at IS NULL AND next_attempt_at <= ?3
              AND (claimed_until IS NULL OR claimed_until < ?3)
            ORDER BY next_attempt_at, work_id
            LIMIT ?4)
         RETURNING work_id, purchase_id, kind, attempts",
    )
    .bind(&worker)
    .bind(now + policy.lease.as_secs() as i64)
    .bind(now)
    .bind(policy.batch)
    .fetch_all(db)
    .await?;

    let cmd = Command::new(executor, db.clone());
    let mut pass = PurchasePass::default();
    for row in rows {
        let Some(kind) = WorkKind::parse(&row.kind) else {
            close(db, &row.work_id, Some("unknown work kind")).await?;
            pass.failed += 1;
            continue;
        };
        let Some(purchase) = cmd.load_purchase(&row.purchase_id).await? else {
            close(db, &row.work_id, Some("purchase is gone")).await?;
            pass.failed += 1;
            continue;
        };
        let Some(supplier) = cmd.load_supplier(&purchase.supplier_id).await? else {
            close(db, &row.work_id, Some("supplier is gone")).await?;
            pass.failed += 1;
            continue;
        };
        let Some(connector) = connectors.of(&supplier.connector) else {
            // Nobody answers for this supplier: the operator buys by hand.
            close(db, &row.work_id, Some("no connector for this supplier")).await?;
            pass.failed += 1;
            continue;
        };
        let task = match kind {
            WorkKind::Place => ConnectorTask::Placing,
            WorkKind::Track | WorkKind::Cancel => ConnectorTask::Tracking,
        };
        if !connector.does(task) {
            close(db, &row.work_id, Some("worked by hand")).await?;
            pass.failed += 1;
            continue;
        }

        let outcome = match kind {
            WorkKind::Place => {
                place_one(
                    &cmd,
                    db,
                    connector.as_ref(),
                    &row,
                    &purchase,
                    policy,
                    &mut pass,
                )
                .await
            }
            WorkKind::Track => {
                track_one(
                    &cmd,
                    db,
                    connector.as_ref(),
                    &row,
                    &purchase,
                    policy,
                    &mut pass,
                )
                .await
            }
            WorkKind::Cancel => {
                cancel_one(db, connector.as_ref(), &row, &purchase, &mut pass).await
            }
        };
        if let Err(err) = outcome {
            tracing::error!(work_id = %row.work_id, %err, "purchase work failed");
            postpone(
                db,
                &row.work_id,
                &worker,
                now,
                policy.wait_after(row.attempts),
                &err.to_string(),
            )
            .await?;
            pass.failed += 1;
        }
    }
    Ok(pass)
}

#[allow(clippy::too_many_arguments)]
async fn place_one<E: Executor>(
    cmd: &Command<'_, E>,
    db: &SqlitePool,
    connector: &dyn crate::connector::SupplierConnector,
    row: &WorkRow,
    purchase: &crate::command::SupplierOrderState,
    policy: &PurchasePolicy,
    pass: &mut PurchasePass,
) -> Result<(), SourcingError> {
    if purchase.status != SupplierOrderStatus::Drafted {
        close(db, &row.work_id, None).await?;
        return Ok(());
    }
    let lines: Vec<crate::connector::PurchaseLine> = purchase
        .lines
        .iter()
        .map(|line| crate::connector::PurchaseLine {
            item: line.item(),
            quantity: line.quantity,
            unit_cost: line.unit_cost.clone(),
        })
        .collect();
    // The purchase's own id is the idempotency key, so a worker that died
    // after the supplier answered buys nothing twice.
    let request = PlaceOrder {
        reference: &purchase.id,
        lines: &lines,
        ship_to: &purchase.ship_to,
        note: None,
    };

    match connector.place(&request).await {
        Ok(placed) => {
            over_budget(&purchase.cost, &placed.cost, policy, &purchase.id);
            cmd.record_supplier_order_placed(&purchase.id, placed.external_order_id, placed.cost)
                .await?;
            close(db, &row.work_id, None).await?;
            enqueue(db, &purchase.id, WorkKind::Track).await?;
            pass.placed += 1;
            Ok(())
        }
        Err(ConnectorError::Refused(reason)) | Err(ConnectorError::UnknownItem(reason)) => {
            cmd.refuse_supplier_order(&purchase.id, reason.clone())
                .await?;
            close(db, &row.work_id, Some(&reason)).await?;
            pass.refused += 1;
            Ok(())
        }
        Err(ConnectorError::RateLimited { retry_after }) => {
            hold(db, &row.work_id, retry_after as i64).await?;
            pass.held += 1;
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn track_one<E: Executor>(
    cmd: &Command<'_, E>,
    db: &SqlitePool,
    connector: &dyn crate::connector::SupplierConnector,
    row: &WorkRow,
    purchase: &crate::command::SupplierOrderState,
    policy: &PurchasePolicy,
    pass: &mut PurchasePass,
) -> Result<(), SourcingError> {
    let Some(reference) = purchase.external_order_id.as_deref() else {
        close(db, &row.work_id, Some("nothing placed to track")).await?;
        return Ok(());
    };
    if purchase.status != SupplierOrderStatus::Placed {
        close(db, &row.work_id, None).await?;
        return Ok(());
    }

    match connector.standing(reference).await {
        Ok(crate::connector::SupplierOrderStanding::Pending) => {
            reschedule(db, &row.work_id, policy.track_every).await?;
            pass.pending += 1;
            Ok(())
        }
        Ok(crate::connector::SupplierOrderStanding::Shipped {
            carrier,
            tracking_number,
        }) => {
            cmd.record_supplier_order_shipped(&purchase.id, carrier, tracking_number)
                .await?;
            close(db, &row.work_id, None).await?;
            pass.shipped += 1;
            Ok(())
        }
        Ok(crate::connector::SupplierOrderStanding::Cancelled { reason }) => {
            cmd.cancel_supplier_order(&purchase.id, reason.clone())
                .await?;
            close(db, &row.work_id, Some(&reason)).await?;
            pass.cancelled += 1;
            Ok(())
        }
        Err(ConnectorError::RateLimited { retry_after }) => {
            hold(db, &row.work_id, retry_after as i64).await?;
            pass.held += 1;
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

async fn cancel_one(
    db: &SqlitePool,
    connector: &dyn crate::connector::SupplierConnector,
    row: &WorkRow,
    purchase: &crate::command::SupplierOrderState,
    pass: &mut PurchasePass,
) -> Result<(), SourcingError> {
    let Some(reference) = purchase.external_order_id.as_deref() else {
        close(db, &row.work_id, None).await?;
        return Ok(());
    };
    match connector.cancel(reference).await {
        Ok(()) => {
            close(db, &row.work_id, None).await?;
            pass.cancelled += 1;
            Ok(())
        }
        // A supplier that will not be told is the operator's problem now: the
        // shop has already stopped meaning to buy it, and somebody has to
        // deal with a parcel that may still arrive.
        Err(ConnectorError::Refused(reason)) => {
            tracing::warn!(
                purchase_id = %purchase.id,
                %reason,
                "the supplier would not call the order off: see to it by hand"
            );
            close(db, &row.work_id, Some(&reason)).await?;
            pass.failed += 1;
            Ok(())
        }
        Err(ConnectorError::RateLimited { retry_after }) => {
            hold(db, &row.work_id, retry_after as i64).await?;
            pass.held += 1;
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// Says so when a supplier charged more than it quoted. The purchase stands:
/// a visible loss of a few cents beats an order nobody ships.
fn over_budget(
    drafted: &timada_core::Money,
    charged: &timada_core::Money,
    policy: &PurchasePolicy,
    purchase_id: &str,
) {
    if drafted.currency != charged.currency || drafted.minor <= 0 {
        return;
    }
    let over = charged.minor - drafted.minor;
    if over <= 0 {
        return;
    }
    let bp = over * 10_000 / drafted.minor;
    if bp > i64::from(policy.cost_tolerance_bp) {
        tracing::warn!(
            %purchase_id,
            drafted = drafted.minor,
            charged = charged.minor,
            over_bp = bp,
            "the supplier charged more than it quoted"
        );
    }
}

async fn reschedule(db: &SqlitePool, work_id: &str, wait: Duration) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sourcing_purchase_work
         SET next_attempt_at = ?2, claimed_by = NULL, claimed_until = NULL
         WHERE work_id = ?1",
    )
    .bind(work_id)
    .bind(timada_core::time::now_unix_secs().unwrap_or_default() as i64 + wait.as_secs() as i64)
    .execute(db)
    .await?;
    Ok(())
}

/// Being throttled costs the row no attempt: it is not the row's fault.
async fn hold(db: &SqlitePool, work_id: &str, retry_after: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sourcing_purchase_work
         SET next_attempt_at = ?2, last_error = 'rate limited',
             claimed_by = NULL, claimed_until = NULL
         WHERE work_id = ?1",
    )
    .bind(work_id)
    .bind(timada_core::time::now_unix_secs().unwrap_or_default() as i64 + retry_after)
    .execute(db)
    .await?;
    Ok(())
}

async fn postpone(
    db: &SqlitePool,
    work_id: &str,
    worker: &str,
    now: i64,
    wait: Duration,
    last_error: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sourcing_purchase_work
         SET attempts = attempts + 1, next_attempt_at = ?3, last_error = ?4,
             claimed_by = NULL, claimed_until = NULL
         WHERE work_id = ?1 AND claimed_by = ?2",
    )
    .bind(work_id)
    .bind(worker)
    .bind(now + wait.as_secs() as i64)
    .bind(last_error)
    .execute(db)
    .await?;
    Ok(())
}

fn worker_id() -> String {
    static PASSES: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
        PASSES.fetch_add(1, Ordering::Relaxed)
    )
}

/// Runs [`work_purchases_with`] every `every`, forever.
pub async fn run_purchases<E: Executor>(
    executor: E,
    db: SqlitePool,
    connectors: SupplierConnectors,
    every: Duration,
) {
    let policy = PurchasePolicy::default();
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match work_purchases_with(&executor, &db, &connectors, &policy).await {
            Ok(pass) if pass != PurchasePass::default() => {
                tracing::info!(?pass, "supplier purchase pass")
            }
            Ok(_) => {}
            Err(err) => tracing::error!(error = %err, "supplier purchase pass failed"),
        }
    }
}
