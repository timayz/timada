//! Anti-corruption layers: orders drive the invoice lifecycle, and the
//! payment context's refunds are documented by credit notes. Not strict —
//! each listens to a subset of a foreign aggregate.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_order::{
    PromoKind,
    aggregator::{OrderCancelled, OrderPaid, OrderPlaced, OrderSettled},
};

use timada_payment::aggregator::PaymentRefunded;

use crate::{
    command::{Command, DraftInvoice, IssueCreditNote, invoice_id},
    error::InvoiceError,
    query::load_invoice,
    value_object::{InvoiceDiscount, InvoiceLine, InvoiceStatus, InvoiceTax},
};

pub const INVOICE_FROM_ORDERS_SUBSCRIPTION: &str = "invoice-from-orders";
pub const CREDIT_NOTES_FROM_REFUNDS_SUBSCRIPTION: &str = "invoice-credit-notes-from-refunds";

/// Needs the `SqlitePool` as subscription data: credit note numbers are
/// allocated in SQL.
pub fn credit_notes_from_refunds_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CREDIT_NOTES_FROM_REFUNDS_SUBSCRIPTION)
        .handler(credit_on_payment_refunded())
}

pub fn invoice_from_orders_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(INVOICE_FROM_ORDERS_SUBSCRIPTION)
        .handler(draft_on_order_placed())
        .handler(issue_on_order_paid())
        .handler(issue_on_order_settled())
        .handler(void_on_order_cancelled())
}

fn command<'a, E: Executor>(ctx: &'a Context<'_, E>) -> anyhow::Result<Command<'a, E>> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    Ok(Command {
        executor: ctx.executor,
        db,
    })
}

#[evento::subscription]
async fn draft_on_order_placed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPlaced>,
) -> anyhow::Result<()> {
    // The discount is its own order event, committed together with this one.
    let Some(order) = timada_order::load_order_details(ctx.executor, &event.aggregate_id).await?
    else {
        anyhow::bail!("order {} placed but cannot be loaded", event.aggregate_id);
    };
    let discount = order.discount.map(|d| InvoiceDiscount {
        label: match d.kind {
            PromoKind::Discount => format!("Code promo {}", d.code),
            PromoKind::Voucher => format!("Bon d'achat {}", d.code),
        },
        amount: d.amount,
    });

    command(ctx)?
        .draft_invoice(DraftInvoice {
            order_id: event.aggregate_id.to_owned(),
            customer_id: event.data.customer_id,
            billing_address: event.data.billing_address,
            lines: event
                .data
                .lines
                .into_iter()
                .map(|l| InvoiceLine {
                    product_id: l.product_id,
                    label: l.name,
                    quantity: l.quantity,
                    unit_price: l.unit_price,
                })
                .collect(),
            shipping_fee: event.data.shipping_fee,
            handling_fee: event.data.handling_fee,
            discount,
            tax: order.tax.map(|tax| InvoiceTax {
                zone_code: tax.zone_code,
                treatment: tax.treatment,
                vat_lines: tax.vat_lines,
            }),
        })
        .await?;
    Ok(())
}

#[evento::subscription]
async fn issue_on_order_paid<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPaid>,
) -> anyhow::Result<()> {
    // A missing draft is an ordering glitch: fail so the subscription retries.
    command(ctx)?
        .issue_invoice(invoice_id(&event.aggregate_id))
        .await?;
    Ok(())
}

/// An order with nothing left to pay is invoiced like a paid one.
#[evento::subscription]
async fn issue_on_order_settled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderSettled>,
) -> anyhow::Result<()> {
    command(ctx)?
        .issue_invoice(invoice_id(&event.aggregate_id))
        .await?;
    Ok(())
}

/// A draft is voided. An issued invoice is left alone: it cannot be edited,
/// and the refund that follows the cancellation is documented by a credit
/// note — unless nothing was paid, in which case there is nothing to credit
/// and the invoice is voided too.
#[evento::subscription]
async fn void_on_order_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    let id = invoice_id(&event.aggregate_id);
    let Some(invoice) = load_invoice(ctx.executor, &id).await? else {
        return Ok(());
    };
    if invoice.status == InvoiceStatus::Issued && invoice.total.is_positive() {
        return Ok(());
    }
    command(ctx)?.void_invoice(id, "order cancelled").await?;
    Ok(())
}

/// `PaymentRefunded` → a credit note against the order's invoice, one per
/// refund (the id is derived from the refund event's id). An invoice that is
/// not issued yet is an ordering glitch: fail so the subscription retries.
#[evento::subscription]
async fn credit_on_payment_refunded<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<PaymentRefunded>,
) -> anyhow::Result<()> {
    let Some(payment) = timada_payment::load_payment(ctx.executor, &event.aggregate_id).await?
    else {
        anyhow::bail!(
            "payment {} refunded but cannot be loaded",
            event.aggregate_id
        );
    };
    let issued = command(ctx)?
        .issue_credit_note(IssueCreditNote {
            refund_id: event.id.to_string(),
            invoice_id: invoice_id(&payment.order_id),
            amount: event.data.amount,
            reason: event.data.reason,
        })
        .await;
    match issued {
        Ok(_) => Ok(()),
        // Cancelled before it was paid, then refunded a late capture: the
        // voided invoice never billed anything, there is nothing to credit.
        Err(InvoiceError::InvoiceVoided) => {
            tracing::warn!(order_id = %payment.order_id, "refund on a voided invoice: no credit note");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}
