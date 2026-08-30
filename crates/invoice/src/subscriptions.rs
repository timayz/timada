//! The `invoice-issuance` subscription: invoices follow orders, automatically.
//!
//! ```text
//! OrderPaid      → issue_invoice      → InvoiceIssued
//! OrderCancelled → issue_credit_note  → CreditNoteIssued (only if invoiced)
//! ReturnRefunded → issue_credit_note  → CreditNoteIssued (only if invoiced)
//! ```
//!
//! It lives here and not in the order context on purpose: billing is the
//! invoice context's business, and an order should not have to know it is
//! being invoiced. The coupling runs one way, through published order events.
//!
//! **Both handlers are on one subscription, deliberately.** A refunded order
//! emits `OrderPaid` and then `OrderCancelled`, and the credit note only exists
//! if the invoice does. One subscription processes an aggregate's events in
//! version order, so the invoice is always issued before the cancellation is
//! seen. Splitting these across two subscriptions would race, and the loser
//! would silently drop the credit note.
//!
//! **Why this subscription is not `.strict()`.** It handles two of the order
//! aggregate's seven events. Without strict mode evento derives its read filter
//! from the registered handlers and fetches only those two; strict mode would
//! instead pull in every order event and then fail on the five it ignores —
//! the same reasoning as the order context's own saga.
//!
//! **Idempotency.** A retry resumes from the last acknowledged event and a
//! mid-handler failure replays the whole handler, so both commands are
//! no-ops on a second pass: `issue_invoice` checks for `InvoiceIssued` before
//! it allocates a number, `issue_credit_note` checks the replayed invoice for a
//! credit note it already carries.

use evento::metadata::Event;
use evento::subscription::{Context, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::Executor;
use timada_order::{OrderCancelled, OrderPaid, load_order};
use timada_return::ReturnRefunded;

use crate::commands::{issue_credit_note, issue_invoice};
use crate::state::InvoiceConfig;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const ISSUANCE_SUBSCRIPTION: &str = "invoice-issuance";

/// The issuance subscription, unstarted.
///
/// Handlers must be generic over the executor (the macro requires it) but the
/// commands they dispatch need the concrete framework [`Executor`], the write
/// pool the number counter lives on, and the seller — so all three are injected
/// as subscription data.
pub fn issuance_subscription(
    executor: Executor,
    write_pool: SqlitePool,
    config: InvoiceConfig,
) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(ISSUANCE_SUBSCRIPTION)
        .data(executor)
        .data(write_pool)
        .data(config)
        .handler(on_order_paid())
        .handler(on_order_cancelled())
        .handler(on_return_refunded())
}

fn missing(what: &str) -> anyhow::Error {
    anyhow::anyhow!("`{ISSUANCE_SUBSCRIPTION}` was started without {what}")
}

fn executor<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<Executor> {
    ctx.get::<Executor>().ok_or_else(|| missing("an executor"))
}

fn write_pool<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| missing("a write pool"))
}

fn config<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<InvoiceConfig> {
    ctx.get::<InvoiceConfig>()
        .ok_or_else(|| missing("an invoice config"))
}

/// The money landed, so there is something to bill for.
#[evento::subscription]
async fn on_order_paid<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPaid>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    // An order that cannot be loaded is a read that raced its own write, not a
    // missing order — bail so the retry covers it rather than skipping the
    // invoice for good.
    let Some(order) = load_order(&executor, &event.aggregate_id).await? else {
        anyhow::bail!("paid order {} cannot be loaded", event.aggregate_id);
    };

    issue_invoice(&executor, &write_pool(ctx)?, &config(ctx)?, &order).await?;
    Ok(())
}

/// The sale came undone. If it had been invoiced, the invoice is reversed;
/// if it never got that far — a declined charge — this does nothing.
#[evento::subscription]
async fn on_order_cancelled<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    issue_credit_note(
        &executor(ctx)?,
        &write_pool(ctx)?,
        &config(ctx)?,
        &event.aggregate_id,
        &event.data.reason,
    )
    .await
}

/// A delivered order came back and was refunded in full — the invoice is
/// reversed. `ReturnRefunded` carries the order id precisely so this handler
/// never has to replay the return.
#[evento::subscription]
async fn on_return_refunded<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnRefunded>,
) -> anyhow::Result<()> {
    issue_credit_note(
        &executor(ctx)?,
        &write_pool(ctx)?,
        &config(ctx)?,
        &event.data.order_id,
        "order returned",
    )
    .await
}
