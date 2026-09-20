//! The `return-processing` process manager: once a parcel is received, put
//! the accepted units back into stock, give the customer their money and/or
//! store credit — or send the same products again, when the operator chose a
//! replacement — and complete the return.
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
use timada_inventory::{
    InventoryError, RegisterStockItem, ReservationOutcome, StockLocation, stock_item_id,
};
use timada_payment::{PaymentError, payment_id};
use timada_promotion::{IssueVoucher, PromotionError, VoucherKind};
use timada_shipping::{CreateShipment, DeliveryMethod, ShipmentLine};

use crate::{
    aggregator::ReturnReceived,
    command::Command,
    query::{ReturnView, load_return},
    value_object::{ReplacementStatus, ReturnPolicy, ReturnStatus},
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

    // 2. A replacement instead of a refund: the units are put aside, a
    //    parcel is prepared, and the return is done. Should the stock be gone
    //    by now, the return goes on as the refund settled as its fallback.
    let commands = Command {
        executor: ctx.executor,
        db: db.clone(),
        policy: ReturnPolicy::default(),
    };
    let mut money = event.data.money.clone();
    let mut credit = event.data.credit.clone();
    if let Some(replacement) = &request.replacement {
        if replacement.status == ReplacementStatus::Planned {
            match arrange_replacement(ctx.executor, &request).await? {
                Ok(shipment_id) => {
                    let nothing = Money::zero(&money.currency);
                    commands
                        .complete_return(
                            &request.id,
                            nothing.clone(),
                            nothing,
                            None,
                            Some(shipment_id),
                        )
                        .await?;
                    return Ok(());
                }
                Err(reason) => commands.abandon_replacement(&request.id, reason).await?,
            }
        }
        money = replacement.fallback_money.clone();
        credit = replacement.fallback_credit.clone();
    }

    // 3. Money back to the original payment. Should the payment refuse after
    //    all (refunded by hand in the meantime), the amount becomes credit.
    let currency = money.currency.clone();
    let mut refunded = Money::zero(&currency);
    if money.is_positive() {
        let outcome = timada_payment::Command(ctx.executor)
            .refund_payment_once(
                payment_id(&request.order_id),
                refund_reference(&request.rma_number),
                money.clone(),
            )
            .await;
        match outcome {
            Ok(_) => refunded = money.clone(),
            Err(
                PaymentError::RefundExceedsCapture
                | PaymentError::NotCaptured
                | PaymentError::PaymentNotFound,
            ) => {
                tracing::warn!(return_id = %request.id, "payment cannot be refunded: store credit instead");
                credit = credit.checked_add(&money)?;
            }
            Err(err) => return Err(err.into()),
        }
    }

    // 4. Store credit, as a voucher whose id derives from its code.
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

    commands
        .complete_return(&request.id, refunded, credit, code, None)
        .await?;
    Ok(())
}

/// Puts the replacement's units aside — under the return's id, so a retry
/// reserves nothing twice — and prepares its parcel, to the order's delivery
/// address by the order's delivery method. `Ok(Err(reason))` when it cannot be
/// done: whatever was reserved is given back first.
async fn arrange_replacement<E: Executor>(
    executor: &E,
    request: &ReturnView,
) -> anyhow::Result<Result<String, String>> {
    let Some(replacement) = &request.replacement else {
        anyhow::bail!("return {} has no replacement to arrange", request.id);
    };
    let Some(order) = timada_order::load_order_details(executor, &request.order_id).await? else {
        anyhow::bail!("order {} of a return cannot be loaded", request.order_id);
    };
    let Some(method) = DeliveryMethod::resolve(
        &order.delivery.method_code,
        order.delivery.pickup_store_id.clone(),
    ) else {
        return Ok(Err(format!(
            "delivery method {} is no longer offered",
            order.delivery.method_code
        )));
    };

    let inventory = timada_inventory::Command(executor);
    let mut short = None;
    for line in &replacement.lines {
        let item = stock_item_id(&line.product_id, &StockLocation::Warehouse);
        match inventory
            .reserve_stock(&item, &request.id, line.quantity)
            .await
        {
            Ok(ReservationOutcome::Reserved) => {}
            Ok(ReservationOutcome::Rejected { .. }) | Err(InventoryError::StockItemNotFound) => {
                short = Some(line.product_id.clone());
                break;
            }
            Err(err) => return Err(err.into()),
        }
    }
    if let Some(product_id) = short {
        for line in &replacement.lines {
            let item = stock_item_id(&line.product_id, &StockLocation::Warehouse);
            match inventory.release_stock(&item, &request.id).await {
                Ok(()) | Err(InventoryError::StockItemNotFound) => {}
                Err(err) => return Err(err.into()),
            }
        }
        return Ok(Err(format!("out of stock: {product_id}")));
    }

    let shipment_id = timada_shipping::Command(executor)
        .create_replacement_shipment(
            CreateShipment {
                order_id: request.order_id.clone(),
                method,
                destination: order.delivery_address,
                lines: replacement
                    .lines
                    .iter()
                    .map(|line| ShipmentLine {
                        product_id: line.product_id.clone(),
                        quantity: line.quantity,
                    })
                    .collect(),
            },
            &request.id,
        )
        .await?;
    Ok(Ok(shipment_id))
}
