//! The `return-processing` process manager: once a parcel is received, put
//! the accepted units back into stock, give the customer their money and/or
//! store credit, and complete the return.
//!
//! Every step carries its own idempotency key — the return id for the
//! restock, the RMA number for the refund and the voucher — so a redelivery
//! after a crash redoes nothing. Not strict: it listens to one event.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_core::Money;
use timada_inventory::{InventoryError, RegisterStockItem, StockLocation, stock_item_id};
use timada_payment::{PaymentError, payment_id};
use timada_promotion::{IssueVoucher, PromotionError, VoucherKind};

use crate::{
    aggregator::ReturnReceived,
    command::Command,
    query::load_return,
    value_object::{ReturnPolicy, ReturnStatus},
};

pub const RETURN_PROCESSING_SUBSCRIPTION: &str = "return-processing";

/// Needs the `SqlitePool` as subscription data: vouchers and return claims
/// live in SQL.
pub fn return_processing_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(RETURN_PROCESSING_SUBSCRIPTION).handler(process_received_return())
}

/// The refund reason recorded on the payment, and the refund's idempotency key.
pub fn refund_reference(rma_number: &str) -> String {
    format!("return {rma_number}")
}

/// The store-credit voucher a return issues.
pub fn voucher_code(rma_number: &str) -> String {
    format!("AVOIR-{rma_number}")
}

#[evento::subscription]
async fn process_received_return<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnReceived>,
) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(request) = load_return(ctx.executor, &event.aggregate_id).await? else {
        anyhow::bail!(
            "return {} received but cannot be loaded",
            event.aggregate_id
        );
    };
    if request.status != ReturnStatus::Received {
        return Ok(());
    }

    // 1. Back on the shelf — returns always come back to the warehouse.
    let inventory = timada_inventory::Command(ctx.executor);
    for line in event
        .data
        .lines
        .iter()
        .filter(|l| l.restock && l.accepted > 0)
    {
        let item = stock_item_id(&line.product_id, &StockLocation::Warehouse);
        let restocked = inventory
            .restock_return(&item, &request.id, line.accepted)
            .await;
        match restocked {
            Ok(()) => {}
            // Never stocked at the warehouse (sold from a shop): start tracking it.
            Err(InventoryError::StockItemNotFound) => {
                let registered = inventory
                    .register_stock_item(RegisterStockItem {
                        product_id: line.product_id.clone(),
                        location: StockLocation::Warehouse,
                    })
                    .await;
                match registered {
                    Ok(_) | Err(InventoryError::AlreadyRegistered) => {}
                    Err(err) => return Err(err.into()),
                }
                inventory
                    .restock_return(&item, &request.id, line.accepted)
                    .await?;
            }
            Err(err) => return Err(err.into()),
        }
    }

    // 2. Money back to the original payment. Should the payment refuse after
    //    all (refunded by hand in the meantime), the amount becomes credit.
    let currency = event.data.money.currency.clone();
    let mut refunded = Money::zero(&currency);
    let mut credit = event.data.credit.clone();
    if event.data.money.is_positive() {
        let outcome = timada_payment::Command(ctx.executor)
            .refund_payment_once(
                payment_id(&request.order_id),
                refund_reference(&request.rma_number),
                event.data.money.clone(),
            )
            .await;
        match outcome {
            Ok(_) => refunded = event.data.money.clone(),
            Err(
                PaymentError::RefundExceedsCapture
                | PaymentError::NotCaptured
                | PaymentError::PaymentNotFound,
            ) => {
                tracing::warn!(return_id = %request.id, "payment cannot be refunded: store credit instead");
                credit = credit.checked_add(&event.data.money)?;
            }
            Err(err) => return Err(err.into()),
        }
    }

    // 3. Store credit, as a voucher whose id derives from its code.
    let mut code = None;
    if credit.is_positive() {
        let voucher = voucher_code(&request.rma_number);
        let issued = timada_promotion::Command {
            executor: ctx.executor,
            db: db.clone(),
        }
        .issue_voucher(IssueVoucher {
            code: voucher.clone(),
            customer_id: Some(request.customer_id.clone()),
            value: credit.clone(),
            kind: VoucherKind::CreditNote {
                origin_order_id: request.order_id.clone(),
            },
            expires_at: None,
        })
        .await;
        match issued {
            Ok(_) | Err(PromotionError::CodeAlreadyExists(_)) => code = Some(voucher),
            Err(err) => return Err(err.into()),
        }
    }

    Command {
        executor: ctx.executor,
        db,
        policy: ReturnPolicy::default(),
    }
    .complete_return(&request.id, refunded, credit, code)
    .await?;
    Ok(())
}
