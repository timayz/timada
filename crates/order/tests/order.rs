//! End-to-end: cart checkout → order placed → stock reserved → payment
//! requested/captured → shipment created/dispatched → order shipped, plus
//! the two compensation branches (out of stock, payment declined).

use evento::Executor;
use timada_cart::{AddLine, Checkout};
use timada_core::{Address, Money};
use timada_inventory::{RegisterStockItem, StockLocation, stock_item_id};
use timada_order::{
    FulfillmentStatus, ListOrders, OrderStatus, PaymentMode, count_orders, history, list_orders,
    load_fulfillment, load_order_details, migrations, order_checkout_subscription,
    order_fulfillment_subscription, order_history_subscription, order_id, orders_of_customer,
};
use timada_payment::payment_id;
use timada_shipping::shipment_id;

const PRODUCT: &str = "aoc-24g4xe";
const CUSTOMER: &str = "customer-1";

fn address(line1: &str, postal_code: &str, city: &str, country: &str) -> Address {
    Address {
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: line1.into(),
        postal_code: postal_code.into(),
        city: city.into(),
        country_code: country.into(),
        ..Address::default()
    }
}

/// Stocks `on_hand` units in the warehouse and checks out a cart of `quantity`.
async fn checkout_cart<E: Executor>(
    executor: &E,
    on_hand: u32,
    quantity: u32,
    payment_mode: timada_cart::PaymentMode,
) -> anyhow::Result<String> {
    let inventory = timada_inventory::Command(executor);
    let stock = inventory
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Warehouse,
        })
        .await?;
    if on_hand > 0 {
        inventory.receive_stock(&stock, on_hand).await?;
    }

    let cart = timada_cart::Command(executor);
    let cart_id = cart.open_cart(Some(CUSTOMER.into())).await?;
    cart.add_line(
        &cart_id,
        AddLine {
            product_id: PRODUCT.into(),
            name: "AOC 23.8\" LED - 24G4XE".into(),
            quantity,
            unit_price: Money::eur(12_496),
            warranty_months: 60,
        },
    )
    .await?;
    cart.checkout(
        &cart_id,
        Checkout {
            customer_id: None,
            delivery_address: address("La agnès", "97290", "Le Marin", "MQ"),
            billing_address: address("121, Avenue Tolosane", "31520", "Ramonville", "FR"),
            delivery: timada_cart::DeliveryChoice {
                method_code: "chronopost-dom".into(),
                pickup_store_id: None,
            },
            payment_mode,
        },
    )
    .await?;
    Ok(cart_id)
}

/// Drains the checkout ACL and the saga until they stop producing events.
async fn drain<E: Executor + Clone + 'static>(executor: &E) -> anyhow::Result<()> {
    for _ in 0..4 {
        order_checkout_subscription().run_once(executor).await?;
        order_fulfillment_subscription().run_once(executor).await?;
    }
    Ok(())
}

#[tokio::test]
async fn cart_checkout_is_fulfilled_through_payment_and_shipping() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(
        &executor,
        5,
        2,
        timada_cart::PaymentMode::Installments { count: 3 },
    )
    .await?;
    let order_id = order_id(&cart_id);

    // Checkout → order placed → stock reserved → payment requested.
    drain(&executor).await?;
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    assert_eq!(order.status, OrderStatus::Placed);
    assert_eq!(order.customer_id, CUSTOMER);
    assert_eq!(order.payment_mode, PaymentMode::Installments { count: 3 });
    assert_eq!(order.subtotal, Money::eur(24_992));
    assert_eq!(order.shipping_fee, Money::eur(2_395));
    assert_eq!(order.handling_fee, Money::eur(449));
    assert_eq!(order.total, Money::eur(27_836));

    let saga = load_fulfillment(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("saga not started"))?;
    assert_eq!(saga.status, FulfillmentStatus::AwaitingPayment);
    assert_eq!(
        saga.payment_id.as_deref(),
        Some(payment_id(&order_id).as_str())
    );

    let stock = timada_inventory::load_stock_availability(
        &executor,
        stock_item_id(PRODUCT, &StockLocation::Warehouse),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("stock item missing"))?;
    assert_eq!((stock.reserved, stock.available), (2, 3));

    let payment = timada_payment::load_payment(&executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment not requested"))?;
    assert_eq!(payment.amount, Money::eur(27_836));

    // PSP captures → order paid, shipment created.
    timada_payment::Command(&executor)
        .capture_payment(payment_id(&order_id), "psp-123".into())
        .await?;
    drain(&executor).await?;
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.status, OrderStatus::Paid);
    let shipment = timada_shipping::load_shipment(&executor, shipment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("shipment not created"))?;
    assert_eq!(shipment.destination.country_code, "MQ");
    assert_eq!(shipment.lines.len(), 1);

    // Carrier picks up → order shipped, saga completed.
    timada_shipping::Command(&executor)
        .dispatch_shipment(shipment_id(&order_id), "Chronopost".into(), "XY123".into())
        .await?;
    drain(&executor).await?;
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.status, OrderStatus::Shipped);
    assert_eq!(order.tracking_number.as_deref(), Some("XY123"));
    assert!(order.shipped_at.is_some());
    let saga = load_fulfillment(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("saga missing"))?;
    assert_eq!(saga.status, FulfillmentStatus::Completed);

    // Redelivering everything changes nothing.
    drain(&executor).await?;
    let again = load_order_details(&executor, &order_id).await?;
    assert_eq!(again.as_ref(), Some(&order));

    // Order history lists it under the year it was placed.
    order_history_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let year = timada_core::time::year_of(order.placed_at);
    let rows = history(&db, CUSTOMER, year).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "shipped");
    assert_eq!(rows[0].total_minor, 27_836);
    assert!(history(&db, CUSTOMER, year - 1).await?.is_empty());

    // Admin listings across customers and years.
    assert_eq!(list_orders(&db, &ListOrders::default()).await?.len(), 1);
    let cancelled = ListOrders {
        status: Some(OrderStatus::Cancelled),
        ..ListOrders::default()
    };
    assert!(list_orders(&db, &cancelled).await?.is_empty());
    assert_eq!(orders_of_customer(&db, CUSTOMER).await?.len(), 1);
    assert_eq!(count_orders(&db, None).await?, 1);

    Ok(())
}

#[tokio::test]
async fn out_of_stock_cancels_the_order() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 1, 2, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);

    drain(&executor).await?;

    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancelled_reason.as_deref(), Some("out of stock"));
    let saga = load_fulfillment(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("saga missing"))?;
    assert_eq!(saga.status, FulfillmentStatus::Compensated);
    assert!(
        timada_payment::load_payment(&executor, payment_id(&order_id))
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn declined_payment_releases_stock_and_cancels() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 5, 2, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);

    drain(&executor).await?;
    timada_payment::Command(&executor)
        .decline_payment(payment_id(&order_id), "insufficient funds".into())
        .await?;
    drain(&executor).await?;

    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancelled_reason.as_deref(), Some("payment declined"));
    let stock = timada_inventory::load_stock_availability(
        &executor,
        stock_item_id(PRODUCT, &StockLocation::Warehouse),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("stock item missing"))?;
    assert_eq!((stock.reserved, stock.available), (0, 5));
    assert!(
        timada_shipping::load_shipment(&executor, shipment_id(&order_id))
            .await?
            .is_none()
    );
    Ok(())
}
