use evento::{Executor, ProjectionAggregate, metadata::Event, subscription::Context};
use timada_core::Money;
use timada_inventory::aggregator::StockReserved;
use timada_payment::{PaymentMethod, RequestPayment};

use crate::{
    aggregator::{LineStockReserved, PaymentRequested},
    value_object::{FulfillmentStatus, PaymentMode},
};

use super::load_fulfillment;

/// `StockReserved` → tick the line off; once every line is reserved, request
/// the payment. The payment request is idempotent (id derived from the order),
/// so a redelivery cannot charge twice.
#[evento::subscription]
pub(super) async fn record_line_reserved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReserved>,
) -> anyhow::Result<()> {
    let Some(saga) = load_fulfillment(ctx.executor, &event.data.order_id).await? else {
        // A reservation made outside this saga (admin tooling, other flows).
        return Ok(());
    };
    if saga.status != FulfillmentStatus::ReservingStock {
        return Ok(());
    }
    let Some(line) = saga.line_for_stock_item(&event.aggregate_id) else {
        return Ok(());
    };
    if saga.reserved_product_ids.contains(&line.product_id) {
        return Ok(());
    }

    let mut write = saga.write()?;
    write.event(&LineStockReserved {
        product_id: line.product_id.clone(),
    });

    let last_line = saga.lines.iter().all(|l| {
        l.product_id == line.product_id || saga.reserved_product_ids.contains(&l.product_id)
    });
    if last_line {
        let method = match &saga.payment_mode {
            PaymentMode::Card => PaymentMethod::Card,
            // The instalment fee is already part of `amount` (frais de dossier).
            PaymentMode::Installments { count } => PaymentMethod::Installments {
                count: *count,
                fee: Money::zero(&saga.amount.currency),
            },
        };
        let payment_id = timada_payment::Command(ctx.executor)
            .request_payment(RequestPayment {
                order_id: saga.order_id.clone(),
                amount: saga.amount.clone(),
                method,
            })
            .await?;
        write.event(&PaymentRequested { payment_id });
        tracing::info!(order_id = %saga.order_id, "all lines reserved, payment requested");
    }

    write.commit(ctx.executor).await?;
    Ok(())
}
