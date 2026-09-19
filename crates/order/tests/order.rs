//! End-to-end: cart checkout → order placed → stock reserved → payment
//! requested/captured → shipment created/dispatched → order shipped, plus
//! the two compensation branches (out of stock, payment declined) and the
//! cart's promo code: redeemed before the order is placed, given back when
//! the order is cancelled — the payment-less path of an order the code paid
//! for entirely, the refund of an order cancelled after it was paid, the
//! timeout of a payment nobody completes, and resuming a half-started saga.

use evento::Executor;
use timada_cart::{AddLine, Checkout};
use timada_core::{Address, Money};
use timada_inventory::{RegisterStockItem, StockLocation, stock_item_id};
use timada_order::{
    FulfillmentStatus, ListOrders, OrderStatus, PaymentMode, PromoKind, count_orders, history,
    list_orders, load_fulfillment, load_order_details, migrations, order_checkout_subscription,
    order_fulfillment_subscription, order_history_subscription, order_id, order_numbers_by_ids,
    order_promo_release_subscription, orders_awaiting_payment, orders_of_customer,
    payment_deadline_subscription,
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
    checkout_cart_with_code(executor, on_hand, quantity, payment_mode, None).await
}

async fn checkout_cart_with_code<E: Executor>(
    executor: &E,
    on_hand: u32,
    quantity: u32,
    payment_mode: timada_cart::PaymentMode,
    promo_code: Option<&str>,
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
    if let Some(code) = promo_code {
        cart.apply_promo_code(&cart_id, code.into()).await?;
    }
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

/// Drains the checkout ACL (which allocates the order number in SQL) and the
/// saga until they stop producing events.
async fn drain<E: Executor + Clone + 'static>(
    executor: &E,
    db: &sqlx::SqlitePool,
) -> anyhow::Result<()> {
    for _ in 0..4 {
        order_checkout_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
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
    drain(&executor, &db).await?;
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    assert_eq!(order.status, OrderStatus::Placed);
    assert_eq!(order.customer_id, CUSTOMER);
    assert_eq!(order.payment_mode, PaymentMode::Installments { count: 3 });
    // Delivered to Martinique: an export. The listed prices (2 × 124,96 and
    // 23,95 of delivery, French VAT included) are charged without that VAT;
    // the instalment fee never carried any.
    assert_eq!(order.subtotal, Money::eur(20_826));
    assert_eq!(order.shipping_fee, Money::eur(1_996));
    assert_eq!(order.handling_fee, Money::eur(449));
    assert_eq!(order.total, Money::eur(23_271));
    let tax = order
        .tax
        .clone()
        .ok_or_else(|| anyhow::anyhow!("order not taxed"))?;
    assert_eq!(tax.zone_code, "fr-overseas");
    assert_eq!(tax.treatment, timada_tax::TaxTreatment::Export);
    assert_eq!(tax.vat_lines.len(), 1);
    assert_eq!(tax.vat_lines[0].rate_bp, 0);
    assert_eq!(tax.vat_lines[0].total, Money::eur(23_271));
    assert_eq!(tax.vat_total()?, Money::eur(0));

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
    assert_eq!(payment.amount, Money::eur(23_271));

    // PSP captures → order paid, shipment created.
    timada_payment::Command(&executor)
        .capture_payment(payment_id(&order_id), "psp-123".into())
        .await?;
    drain(&executor, &db).await?;
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
    drain(&executor, &db).await?;
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
    drain(&executor, &db).await?;
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
    assert_eq!(rows[0].total_minor, 23_271);
    assert!(history(&db, CUSTOMER, year - 1).await?.is_empty());

    // Admin listings across customers and years.
    assert_eq!(list_orders(&db, &ListOrders::default()).await?.len(), 1);
    let cancelled = ListOrders {
        status: Some(OrderStatus::Cancelled),
        ..ListOrders::default()
    };
    assert!(list_orders(&db, &cancelled).await?.is_empty());
    assert_eq!(orders_of_customer(&db, CUSTOMER).await?.len(), 1);
    assert_eq!(count_orders(&db, &ListOrders::default()).await?, 1);

    // The checkout gave the order a number, found again by its start.
    let year = timada_core::time::year_of(order.placed_at);
    let number = format!("C{year}-000001");
    assert_eq!(order.order_number.as_deref(), Some(number.as_str()));
    assert_eq!(order.display_number(), number);
    assert_eq!(rows[0].order_number.as_deref(), Some(number.as_str()));
    let by_number = ListOrders {
        number: Some(format!("C{year}-")),
        ..ListOrders::default()
    };
    assert_eq!(list_orders(&db, &by_number).await?.len(), 1);
    assert_eq!(count_orders(&db, &by_number).await?, 1);
    let by_id = ListOrders {
        number: Some(order_id.clone()),
        ..ListOrders::default()
    };
    assert_eq!(list_orders(&db, &by_id).await?.len(), 1);
    let unknown = ListOrders {
        number: Some("C1999-".into()),
        ..ListOrders::default()
    };
    assert!(list_orders(&db, &unknown).await?.is_empty());
    let numbers = order_numbers_by_ids(&db, std::slice::from_ref(&order_id)).await?;
    assert_eq!(numbers.get(&order_id), Some(&number));

    Ok(())
}

#[tokio::test]
async fn out_of_stock_cancels_the_order() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 1, 2, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);

    drain(&executor, &db).await?;

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
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 5, 2, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);

    drain(&executor, &db).await?;
    timada_payment::Command(&executor)
        .decline_payment(payment_id(&order_id), "insufficient funds".into())
        .await?;
    drain(&executor, &db).await?;

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

/// Order and promotion tables: the checkout ACL counts redemptions in SQL.
fn migrations_with_promotion() -> Vec<Box<dyn sqlx_migrator::Migration<sqlx::Sqlite>>> {
    let mut all = migrations();
    all.extend(timada_promotion::migrations());
    all
}

/// Like [`drain`], with the pool the promo-code steps need.
async fn drain_with_promotion<E: Executor + Clone + 'static>(
    executor: &E,
    db: &sqlx::SqlitePool,
) -> anyhow::Result<()> {
    for _ in 0..4 {
        order_checkout_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
        order_fulfillment_subscription().run_once(executor).await?;
        order_promo_release_subscription()
            .data(db.clone())
            .run_once(executor)
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn promo_code_lowers_what_is_paid() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations_with_promotion()).await?;
    timada_promotion::Command {
        executor: &executor,
        db: db.clone(),
    }
    .create_discount(timada_promotion::CreateDiscount {
        code: "welcome10".into(),
        kind: timada_promotion::DiscountKind::Percent { bp: 1_000 },
        max_redemptions: Some(1),
        valid_until: None,
    })
    .await?;

    let cart_id = checkout_cart_with_code(
        &executor,
        5,
        2,
        timada_cart::PaymentMode::Card,
        Some("welcome10"),
    )
    .await?;
    let order_id = order_id(&cart_id);
    drain_with_promotion(&executor, &db).await?;

    // 10 % off the goods (249,92 €), not off the shipping fee.
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    let discount = order
        .discount
        .ok_or_else(|| anyhow::anyhow!("discount not applied"))?;
    assert_eq!(discount.code, "WELCOME10");
    assert_eq!(discount.kind, PromoKind::Discount);
    // 10 % of the goods as charged in the zone (export prices, see above).
    assert_eq!(discount.amount, Money::eur(2_083));
    assert_eq!(order.subtotal, Money::eur(20_826));
    assert_eq!(order.total, Money::eur(20_739));

    // The payment and the order history carry the discounted total.
    let payment = timada_payment::load_payment(&executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment not requested"))?;
    assert_eq!(payment.amount, Money::eur(20_739));
    order_history_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let rows = orders_of_customer(&db, CUSTOMER).await?;
    assert_eq!(rows[0].total_minor, 20_739);

    let code = timada_promotion::load_discount_details(
        &executor,
        timada_promotion::discount_id("welcome10"),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("discount missing"))?;
    assert_eq!(code.redeemed, 1);

    // The payment falls through: the order is cancelled and the slot is free.
    timada_payment::Command(&executor)
        .decline_payment(payment_id(&order_id), "insufficient funds".into())
        .await?;
    drain_with_promotion(&executor, &db).await?;
    let code = timada_promotion::load_discount_details(
        &executor,
        timada_promotion::discount_id("welcome10"),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("discount missing"))?;
    assert_eq!(code.redeemed, 0);

    Ok(())
}

#[tokio::test]
async fn voucher_is_spent_on_the_order_and_a_dead_code_is_ignored() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations_with_promotion()).await?;
    let promotion = timada_promotion::Command {
        executor: &executor,
        db: db.clone(),
    };
    let voucher_id = promotion
        .issue_voucher(timada_promotion::IssueVoucher {
            code: "gift50".into(),
            customer_id: None,
            value: Money::eur(5_000),
            kind: timada_promotion::VoucherKind::GiftVoucher,
            expires_at: None,
        })
        .await?;

    let cart_id = checkout_cart_with_code(
        &executor,
        5,
        1,
        timada_cart::PaymentMode::Card,
        Some("gift50"),
    )
    .await?;
    drain_with_promotion(&executor, &db).await?;
    let order = load_order_details(&executor, order_id(&cart_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    let discount = order
        .discount
        .ok_or_else(|| anyhow::anyhow!("voucher not applied"))?;
    assert_eq!(discount.kind, PromoKind::Voucher);
    assert_eq!(discount.amount, Money::eur(5_000));
    assert_eq!(order.total, Money::eur(10_413 + 1_996 - 5_000));
    let voucher = timada_promotion::load_voucher_balance(&executor, &voucher_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("voucher missing"))?;
    assert_eq!(voucher.remaining, Money::eur(0));

    // The voucher is now empty: the next order is placed at full price.
    let cart = timada_cart::Command(&executor);
    let second = cart.open_cart(Some(CUSTOMER.into())).await?;
    cart.add_line(
        &second,
        AddLine {
            product_id: PRODUCT.into(),
            name: "AOC 23.8\" LED - 24G4XE".into(),
            quantity: 1,
            unit_price: Money::eur(12_496),
            warranty_months: 60,
        },
    )
    .await?;
    cart.apply_promo_code(&second, "gift50".into()).await?;
    cart.checkout(
        &second,
        Checkout {
            customer_id: None,
            delivery_address: address("La agnès", "97290", "Le Marin", "MQ"),
            billing_address: address("121, Avenue Tolosane", "31520", "Ramonville", "FR"),
            delivery: timada_cart::DeliveryChoice {
                method_code: "chronopost-dom".into(),
                pickup_store_id: None,
            },
            payment_mode: timada_cart::PaymentMode::Card,
        },
    )
    .await?;
    drain_with_promotion(&executor, &db).await?;
    let order = load_order_details(&executor, order_id(&second))
        .await?
        .ok_or_else(|| anyhow::anyhow!("second order not placed"))?;
    assert_eq!(order.discount, None);
    assert_eq!(order.promo_code.as_deref(), Some("GIFT50"));
    assert_eq!(order.total, Money::eur(10_413 + 1_996));

    Ok(())
}

#[tokio::test]
async fn order_covered_by_a_voucher_skips_the_payment() -> anyhow::Result<()> {
    const STORE: &str = "store-toulouse";
    let (executor, db) = timada_core::testing::memory_executor(migrations_with_promotion()).await?;
    timada_promotion::Command {
        executor: &executor,
        db: db.clone(),
    }
    .issue_voucher(timada_promotion::IssueVoucher {
        code: "gift200".into(),
        customer_id: None,
        value: Money::eur(20_000),
        kind: timada_promotion::VoucherKind::GiftVoucher,
        expires_at: None,
    })
    .await?;

    let inventory = timada_inventory::Command(&executor);
    let stock = inventory
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Store {
                store_id: STORE.into(),
            },
        })
        .await?;
    inventory.receive_stock(&stock, 2).await?;

    // Collected in store: no shipping fee, so the voucher covers everything.
    let cart = timada_cart::Command(&executor);
    let cart_id = cart.open_cart(Some(CUSTOMER.into())).await?;
    cart.add_line(
        &cart_id,
        AddLine {
            product_id: PRODUCT.into(),
            name: "AOC 23.8\" LED - 24G4XE".into(),
            quantity: 1,
            unit_price: Money::eur(12_496),
            warranty_months: 60,
        },
    )
    .await?;
    cart.apply_promo_code(&cart_id, "gift200".into()).await?;
    cart.checkout(
        &cart_id,
        Checkout {
            customer_id: None,
            delivery_address: address("121, Avenue Tolosane", "31520", "Ramonville", "FR"),
            billing_address: address("121, Avenue Tolosane", "31520", "Ramonville", "FR"),
            delivery: timada_cart::DeliveryChoice {
                method_code: "store-pickup".into(),
                pickup_store_id: Some(STORE.into()),
            },
            payment_mode: timada_cart::PaymentMode::Card,
        },
    )
    .await?;
    drain_with_promotion(&executor, &db).await?;

    let order_id = order_id(&cart_id);
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    assert_eq!(order.total, Money::eur(0));
    // Collected in Toulouse: domestic. The voucher paid for the goods, it did
    // not reduce them — the VAT is that of the full price.
    let tax = order
        .tax
        .clone()
        .ok_or_else(|| anyhow::anyhow!("order not taxed"))?;
    assert_eq!(tax.zone_code, "fr");
    assert_eq!(tax.vat_lines.len(), 1);
    assert_eq!(
        (
            tax.vat_lines[0].rate_bp,
            &tax.vat_lines[0].base,
            &tax.vat_lines[0].vat
        ),
        (2_000, &Money::eur(10_413), &Money::eur(2_083))
    );
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(order.payment_id, None);
    assert!(
        timada_payment::load_payment(&executor, payment_id(&order_id))
            .await?
            .is_none()
    );
    let saga = load_fulfillment(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("saga missing"))?;
    assert_eq!(saga.status, FulfillmentStatus::AwaitingShipment);
    assert_eq!(saga.payment_id, None);
    assert!(
        timada_shipping::load_shipment(&executor, shipment_id(&order_id))
            .await?
            .is_some()
    );

    // An order with an amount due cannot be settled by hand.
    let paying = checkout_cart(&executor, 5, 1, timada_cart::PaymentMode::Card).await?;
    drain_with_promotion(&executor, &db).await?;
    let refused = timada_order::Command(&executor)
        .settle_order(timada_order::order_id(&paying))
        .await;
    assert!(matches!(
        refused,
        Err(timada_order::OrderError::AmountDue { .. })
    ));

    // The rest of the saga is unchanged: dispatch → shipped, completed.
    timada_shipping::Command(&executor)
        .dispatch_shipment(shipment_id(&order_id), "LDLC".into(), "PICKUP-1".into())
        .await?;
    drain_with_promotion(&executor, &db).await?;
    let saga = load_fulfillment(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("saga missing"))?;
    assert_eq!(saga.status, FulfillmentStatus::Completed);

    order_history_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let shipped = ListOrders {
        status: Some(OrderStatus::Shipped),
        ..ListOrders::default()
    };
    let rows = list_orders(&db, &shipped).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_minor, 0);
    Ok(())
}

#[tokio::test]
async fn cancelling_a_paid_order_refunds_it_frees_the_stock_and_stops_the_parcel()
-> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 5, 2, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);
    drain(&executor, &db).await?;
    let payments = timada_payment::Command(&executor);
    payments
        .capture_payment(payment_id(&order_id), "psp-123".into())
        .await?;
    drain(&executor, &db).await?;

    // A goodwill gesture first, then the operator cancels the order.
    payments
        .refund_payment(payment_id(&order_id), Money::eur(1_000), "goodwill".into())
        .await?;
    timada_order::Command(&executor)
        .cancel_order(&order_id, "customer changed mind")
        .await?;
    drain(&executor, &db).await?;

    let payment = timada_payment::load_payment(&executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.status, timada_payment::PaymentStatus::Refunded);
    assert_eq!(payment.refunded, payment.amount);
    let stock = timada_inventory::load_stock_availability(
        &executor,
        stock_item_id(PRODUCT, &StockLocation::Warehouse),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("stock item missing"))?;
    assert_eq!((stock.reserved, stock.available), (0, 5));
    let saga = load_fulfillment(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("saga missing"))?;
    assert_eq!(saga.status, FulfillmentStatus::Compensated);
    let shipment = timada_shipping::load_shipment(&executor, shipment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("shipment missing"))?;
    assert_eq!(shipment.status, timada_shipping::ShipmentStatus::Cancelled);

    // Redelivery refunds nothing more.
    drain(&executor, &db).await?;
    let again = timada_payment::load_payment(&executor, payment_id(&order_id)).await?;
    assert_eq!(again.as_ref(), Some(&payment));
    Ok(())
}

#[tokio::test]
async fn payment_captured_after_a_cancellation_is_refunded() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 5, 1, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);
    drain(&executor, &db).await?;

    // Cancelled while the payment is pending: nothing to refund yet.
    timada_order::Command(&executor)
        .cancel_order(&order_id, "customer changed mind")
        .await?;
    drain(&executor, &db).await?;
    let saga = load_fulfillment(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("saga missing"))?;
    assert_eq!(saga.status, FulfillmentStatus::Compensated);

    // The PSP confirms the capture anyway: the money goes straight back.
    timada_payment::Command(&executor)
        .capture_payment(payment_id(&order_id), "psp-late".into())
        .await?;
    drain(&executor, &db).await?;
    let payment = timada_payment::load_payment(&executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.status, timada_payment::PaymentStatus::Refunded);
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert!(
        timada_shipping::load_shipment(&executor, shipment_id(&order_id))
            .await?
            .is_none()
    );
    Ok(())
}

/// Drains everything plus the list of orders waiting for their payment.
async fn drain_with_deadlines<E: Executor + Clone + 'static>(
    executor: &E,
    db: &sqlx::SqlitePool,
) -> anyhow::Result<()> {
    drain(executor, db).await?;
    payment_deadline_subscription()
        .data(db.clone())
        .run_once(executor)
        .await
}

#[tokio::test]
async fn a_payment_nobody_completes_times_out_and_frees_the_stock() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let abandoned = checkout_cart(&executor, 5, 2, timada_cart::PaymentMode::Card).await?;
    let abandoned = order_id(&abandoned);
    drain_with_deadlines(&executor, &db).await?;
    let now = timada_core::time::now_unix_secs()?;

    // Requested a moment ago: inside the delay, nothing expires.
    assert_eq!(orders_awaiting_payment(&db, now + 1).await?.len(), 1);
    assert_eq!(
        timada_order::expire_unpaid_orders(&executor, &db, now.saturating_sub(1_800)).await?,
        0
    );

    // Past the delay: the payment is declined, the saga compensates.
    assert_eq!(
        timada_order::expire_unpaid_orders(&executor, &db, now + 1).await?,
        1
    );
    drain_with_deadlines(&executor, &db).await?;
    let order = load_order_details(&executor, &abandoned)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancelled_reason.as_deref(), Some("payment timed out"));
    let stock = timada_inventory::load_stock_availability(
        &executor,
        stock_item_id(PRODUCT, &StockLocation::Warehouse),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("stock item missing"))?;
    assert_eq!((stock.reserved, stock.available), (0, 5));
    assert!(orders_awaiting_payment(&db, now + 1).await?.is_empty());

    // The PSP callback that finally comes in is refused: nothing to refund.
    let late = timada_payment::Command(&executor)
        .capture_payment(payment_id(&abandoned), "psp-late".into())
        .await;
    assert!(matches!(
        late,
        Err(timada_payment::PaymentError::NotRequested)
    ));
    // Sweeping again finds nothing.
    assert_eq!(
        timada_order::expire_unpaid_orders(&executor, &db, now + 1).await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_captured_payment_is_never_timed_out() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 5, 1, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);
    drain_with_deadlines(&executor, &db).await?;

    // Captured, but the saga has not caught up yet: the sweep must leave it.
    timada_payment::Command(&executor)
        .capture_payment(payment_id(&order_id), "psp-1".into())
        .await?;
    let now = timada_core::time::now_unix_secs()?;
    assert_eq!(
        timada_order::expire_unpaid_orders(&executor, &db, now + 1).await?,
        0
    );
    drain_with_deadlines(&executor, &db).await?;
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.status, OrderStatus::Paid);
    assert!(orders_awaiting_payment(&db, now + 1).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_saga_interrupted_while_reserving_is_resumed() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 5, 2, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);
    order_checkout_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let order = load_order_details(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;

    // What a crash right after opening the saga leaves behind: the saga
    // exists, no stock was asked for, and `OrderPlaced` will be redelivered.
    evento::append(timada_order::fulfillment_id(&order_id))
        .event(&timada_order::aggregator::FulfillmentStarted {
            order_id: order_id.clone(),
            lines: vec![timada_order::FulfillmentLine {
                product_id: PRODUCT.into(),
                quantity: 2,
            }],
            pickup_store_id: None,
            amount: order.total.clone(),
            payment_mode: PaymentMode::Card,
        })
        .commit(&executor)
        .await?;

    drain(&executor, &db).await?;
    let saga = load_fulfillment(&executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("saga missing"))?;
    assert_eq!(saga.status, FulfillmentStatus::AwaitingPayment);
    let stock = timada_inventory::load_stock_availability(
        &executor,
        stock_item_id(PRODUCT, &StockLocation::Warehouse),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("stock item missing"))?;
    assert_eq!((stock.reserved, stock.available), (2, 3));
    Ok(())
}
