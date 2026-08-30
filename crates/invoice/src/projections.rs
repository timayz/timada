//! The `admin_invoice_list` read model.
//!
//! One SQL table serving exactly one query shape: the admin invoices list. It
//! is eventually consistent — a handler runs after the event is committed — and
//! nothing that has to be correct *now* reads it. The printable document
//! replays [`load_invoice`](crate::view::load_invoice) instead.

use evento::metadata::Event;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::{Currency, Executor, Money};

use crate::aggregate::{CreditNoteIssued, InvoiceIssued};
use crate::state::InvoiceState;
use crate::subscriptions::issuance_subscription;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const ADMIN_SUBSCRIPTION: &str = "invoice-admin";

/// One row of the admin invoice list.
#[derive(Debug, sqlx::FromRow)]
pub struct AdminInvoiceRow {
    pub id: String,
    pub order_id: String,
    pub number: String,
    pub total_gross_cents: i64,
    /// ISO code; parsed back into a [`Currency`] for display.
    pub currency: String,
    /// `"issued"` or `"credited"`.
    pub status: String,
    pub credit_note_number: Option<String>,
    /// Epoch milliseconds — sub-second precision keeps the list ordered when
    /// several invoices are issued within the same second.
    pub created_at: i64,
}

impl AdminInvoiceRow {
    pub fn total(&self) -> Money {
        Money::new(
            self.total_gross_cents,
            Currency::from_code(&self.currency).unwrap_or_default(),
        )
    }

    pub fn is_credited(&self) -> bool {
        self.status == "credited"
    }
}

/// Newest invoices first, capped so the page stays cheap.
pub async fn recent_invoices(
    read_pool: &SqlitePool,
    limit: i64,
) -> anyhow::Result<Vec<AdminInvoiceRow>> {
    let rows = sqlx::query_as::<_, AdminInvoiceRow>(
        "SELECT id, order_id, number, total_gross_cents, currency, status, \
                credit_note_number, created_at \
         FROM admin_invoice_list \
         ORDER BY created_at DESC, id DESC \
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(read_pool)
    .await?;

    Ok(rows)
}

/// The admin read-model subscription, unstarted.
///
/// Exposed so tests and the demo app's end-to-end run can drive it
/// deterministically with `.no_retry().run_once(&executor)` instead of racing a
/// background task.
pub fn admin_subscription(write_pool: SqlitePool) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(ADMIN_SUBSCRIPTION)
        .data(write_pool)
        .handler(on_invoice_issued())
        .handler(on_credit_note_issued())
        .strict()
}

/// Spawn every background subscription this crate owns: issuance and the admin
/// read model.
///
/// The caller keeps the handles and calls `shutdown()` on them.
pub async fn start_subscriptions(state: &InvoiceState) -> anyhow::Result<Vec<Subscription>> {
    let issuance = issuance_subscription(
        state.ctx.executor.clone(),
        state.ctx.write_pool.clone(),
        state.config.clone(),
    )
    .start(&state.ctx.executor)
    .await?;

    let admin = admin_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    tracing::info!("invoice subscriptions started");
    Ok(vec![issuance, admin])
}

fn write_pool<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>().ok_or_else(|| {
        anyhow::anyhow!("`{ADMIN_SUBSCRIPTION}` subscription was started without a write pool")
    })
}

/// The conflict clause rewrites only what `InvoiceIssued` carries — a replay
/// must not reset a row that a later `CreditNoteIssued` has already moved on.
#[evento::subscription]
async fn on_invoice_issued<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<InvoiceIssued>,
) -> anyhow::Result<()> {
    let created_at = i64::try_from(event.timestamp)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));

    sqlx::query(
        "INSERT INTO admin_invoice_list \
             (id, order_id, number, total_gross_cents, currency, status, created_at) \
         VALUES (?, ?, ?, ?, ?, 'issued', ?) \
         ON CONFLICT(id) DO UPDATE SET \
             order_id = excluded.order_id, \
             number = excluded.number, \
             total_gross_cents = excluded.total_gross_cents, \
             currency = excluded.currency, \
             created_at = excluded.created_at",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.order_id)
    .bind(&event.data.invoice_number)
    .bind(event.data.total_gross.amount_cents)
    .bind(event.data.total_gross.currency.code())
    .bind(created_at)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

/// The row is always there: one subscription replays an aggregate's events in
/// version order, so `InvoiceIssued` was handled first.
#[evento::subscription]
async fn on_credit_note_issued<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<CreditNoteIssued>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE admin_invoice_list SET status = 'credited', credit_note_number = ? WHERE id = ?",
    )
    .bind(&event.data.credit_note_number)
    .bind(&event.aggregate_id)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}
