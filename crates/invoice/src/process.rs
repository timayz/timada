//! Anti-corruption layer from the order context: orders drive the invoice
//! lifecycle. Not strict — it listens to a subset of a foreign aggregate.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_order::aggregator::{OrderCancelled, OrderPaid, OrderPlaced};

use crate::{
    command::{Command, DraftInvoice, invoice_id},
    error::InvoiceError,
    value_object::InvoiceLine,
};

pub const INVOICE_FROM_ORDERS_SUBSCRIPTION: &str = "invoice-from-orders";

pub fn invoice_from_orders_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(INVOICE_FROM_ORDERS_SUBSCRIPTION)
        .handler(draft_on_order_placed())
        .handler(issue_on_order_paid())
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

#[evento::subscription]
async fn void_on_order_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    match command(ctx)?
        .void_invoice(invoice_id(&event.aggregate_id), "order cancelled")
        .await
    {
        Ok(()) | Err(InvoiceError::InvoiceNotFound) => Ok(()),
        Err(err) => Err(err.into()),
    }
}
