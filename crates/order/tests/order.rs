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

/// A monitor listed at 20 % and two books listed at 5,5 %, delivered to
/// Germany by a shop on the EU one-stop shop: French VAT comes off, German
/// VAT goes on — 19 % by default, 7 % for what the host mapped from 5,5 %.
#[tokio::test]
async fn an_eu_delivery_is_charged_the_vat_of_its_destination() -> anyhow::Result<()> {
    const BOOK: &str = "rust-book";
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let pricing = timada_pricing::Command(&executor);
    for (product_id, price, vat_rate_bp) in [(PRODUCT, 12_000, 2_000), (BOOK, 2_110, 550)] {
        pricing
            .list_price(timada_pricing::ListPrice {
                product_id: product_id.into(),
                price_incl_tax: Money::eur(price),
                vat_rate_bp,
                eco_participation: Money::eur(0),
            })
            .await?;
    }

    let cart = timada_cart::Command(&executor);
    let cart_id = cart.open_cart(Some(CUSTOMER.into())).await?;
    for (product_id, name, quantity, price) in [
        (PRODUCT, "AOC 23.8\" LED - 24G4XE", 1, 12_000),
        (BOOK, "The Rust Programming Language", 2, 2_110),
    ] {
        cart.add_line(
            &cart_id,
            AddLine {
                product_id: product_id.into(),
                name: name.into(),
                quantity,
                unit_price: Money::eur(price),
                warranty_months: 0,
            },
        )
        .await?;
    }
    cart.checkout(
        &cart_id,
        Checkout {
            customer_id: None,
            delivery_address: address("Unter den Linden 1", "10117", "Berlin", "DE"),
            billing_address: address("Unter den Linden 1", "10117", "Berlin", "DE"),
            delivery: timada_cart::DeliveryChoice {
                method_code: timada_tax::EU_DELIVERY_METHOD.into(),
                pickup_store_id: None,
            },
            payment_mode: timada_cart::PaymentMode::Card,
        },
    )
    .await?;

    let zones = timada_tax::TaxZones::france_with_eu_oss().with_mapped_rate("de", 550, 700)?;
    order_checkout_subscription()
        .data(db.clone())
        .data(zones)
        .run_once(&executor)
        .await?;

    let order = load_order_details(&executor, order_id(&cart_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    // 120,00 TTC = 100,00 HT → 119,00 ; 21,10 TTC = 20,00 HT → 21,40.
    let unit_prices: Vec<_> = order.lines.iter().map(|l| l.unit_price.clone()).collect();
    assert_eq!(unit_prices, [Money::eur(11_900), Money::eur(2_140)]);
    assert_eq!(order.subtotal, Money::eur(11_900 + 2 * 2_140));
    // Delivery follows: 12,90 TTC = 10,75 HT, + 19 %.
    assert_eq!(order.shipping_fee, Money::eur(1_279));
    assert_eq!(order.total, Money::eur(17_459));

    let tax = order
        .tax
        .ok_or_else(|| anyhow::anyhow!("order not taxed"))?;
    assert_eq!(tax.zone_code, "de");
    assert_eq!(tax.treatment, timada_tax::TaxTreatment::DestinationVat);
    let lines: Vec<_> = tax
        .vat_lines
        .iter()
        .map(|l| (l.rate_bp, l.base.minor, l.vat.minor, l.total.minor))
        .collect();
    assert_eq!(
        lines,
        [(1_900, 11_075, 2_104, 13_179), (700, 4_000, 280, 4_280)]
    );
    assert_eq!(tax.vat_total()?, Money::eur(2_384));
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

/// Refunds and payment sessions live in the payment context's own tables.
fn migrations_with_payment() -> Vec<Box<dyn sqlx_migrator::Migration<sqlx::Sqlite>>> {
    let mut all = migrations();
    all.extend(timada_payment::migrations());
    all
}

/// Hands the refunds that were asked for to the (manual) provider, which
/// settles them at once.
async fn settle_refunds<E: Executor + Clone + 'static>(
    executor: &E,
    db: &sqlx::SqlitePool,
) -> anyhow::Result<timada_payment::RefundPass> {
    timada_payment::refund_execution_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    Ok(timada_payment::execute_pending_refunds(
        executor,
        db,
        &timada_payment::ManualProvider,
        &timada_payment::RefundPolicy::without_delays(),
    )
    .await?)
}

#[tokio::test]
async fn cancelling_a_paid_order_refunds_it_frees_the_stock_and_stops_the_parcel()
-> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations_with_payment()).await?;
    let cart_id = checkout_cart(&executor, 5, 2, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);
    drain(&executor, &db).await?;
    let payments = timada_payment::Command(&executor);
    payments
        .capture_payment(payment_id(&order_id), "psp-123".into())
        .await?;
    drain(&executor, &db).await?;

    // A goodwill gesture first — still on its way to the provider — then the
    // operator cancels the order: only the rest is asked for.
    payments
        .refund_payment(payment_id(&order_id), Money::eur(1_000), "goodwill".into())
        .await?;
    timada_order::Command(&executor)
        .cancel_order(&order_id, "customer changed mind")
        .await?;
    drain(&executor, &db).await?;
    let pending = timada_payment::load_payment(&executor, payment_id(&order_id))
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(pending.refunded, Money::eur(0));
    assert_eq!(pending.pending_refunds()?, pending.amount);
    assert_eq!(settle_refunds(&executor, &db).await?.settled, 2);

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
    assert_eq!(
        settle_refunds(&executor, &db).await?,
        timada_payment::RefundPass::default()
    );
    let again = timada_payment::load_payment(&executor, payment_id(&order_id)).await?;
    assert_eq!(again.as_ref(), Some(&payment));
    Ok(())
}

#[tokio::test]
async fn payment_captured_after_a_cancellation_is_refunded() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations_with_payment()).await?;
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
    assert_eq!(settle_refunds(&executor, &db).await?.settled, 1);
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
    let (executor, db) = timada_core::testing::memory_executor(migrations_with_payment()).await?;
    let abandoned = checkout_cart(&executor, 5, 2, timada_cart::PaymentMode::Card).await?;
    let abandoned = order_id(&abandoned);
    drain_with_deadlines(&executor, &db).await?;
    let now = timada_core::time::now_unix_secs()?;
    // The shopper opened the provider's page, and left.
    let provider = timada_payment::FakeProvider::default();
    let urls = timada_payment::ReturnUrls {
        paid: "https://shop.test/checkout/pay".into(),
    };
    timada_payment::start_payment(&executor, &db, &provider, &payment_id(&abandoned), &urls)
        .await?;

    // Requested a moment ago: inside the delay, nothing expires.
    assert_eq!(orders_awaiting_payment(&db, now + 1).await?.len(), 1);
    assert_eq!(
        timada_order::expire_unpaid_orders(&executor, &db, &provider, now.saturating_sub(1_800))
            .await?,
        0
    );

    // Past the delay: the payment is declined, the saga compensates.
    assert_eq!(
        timada_order::expire_unpaid_orders(&executor, &db, &provider, now + 1).await?,
        1
    );
    // The session was called off first: nobody pays an order being cancelled.
    assert_eq!(
        provider.cancelled(),
        [timada_payment::FakeProvider::session_of(&payment_id(
            &abandoned
        ))]
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
        timada_order::expire_unpaid_orders(&executor, &db, &provider, now + 1).await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_shopper_who_paid_just_before_the_timeout_keeps_their_order() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations_with_payment()).await?;
    let provider = timada_payment::FakeProvider::default();
    let urls = timada_payment::ReturnUrls {
        paid: "https://shop.test/checkout/pay".into(),
    };
    let paid = order_id(&checkout_cart(&executor, 5, 1, timada_cart::PaymentMode::Card).await?);
    drain_with_deadlines(&executor, &db).await?;

    // The shopper is on the provider's page and pays; the provider's own
    // word has not reached the shop yet when the sweep comes by.
    timada_payment::start_payment(&executor, &db, &provider, &payment_id(&paid), &urls).await?;
    provider.mark_paid(&timada_payment::FakeProvider::session_of(&payment_id(
        &paid,
    )));
    let now = timada_core::time::now_unix_secs()?;
    assert_eq!(
        timada_order::expire_unpaid_orders(&executor, &db, &provider, now + 1).await?,
        0
    );
    drain_with_deadlines(&executor, &db).await?;
    let order = load_order_details(&executor, &paid)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.status, OrderStatus::Paid);

    Ok(())
}

#[tokio::test]
async fn a_captured_payment_is_never_timed_out() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations_with_payment()).await?;
    let cart_id = checkout_cart(&executor, 5, 1, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);
    drain_with_deadlines(&executor, &db).await?;

    // Captured, but the saga has not caught up yet: the sweep must leave it.
    timada_payment::Command(&executor)
        .capture_payment(payment_id(&order_id), "psp-1".into())
        .await?;
    let now = timada_core::time::now_unix_secs()?;
    assert_eq!(
        timada_order::expire_unpaid_orders(
            &executor,
            &db,
            &timada_payment::ManualProvider,
            now + 1
        )
        .await?,
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

/// Registers a business, checks its VAT number, and checks a cart of one
/// monitor (120,00 listed, 20 % inside) out to `country`.
async fn business_checkout(
    executor: &evento::Sqlite,
    registry: &timada_tax::FakeValidator,
    email: &str,
    vat_number: &str,
    country: (&str, &str),
) -> anyhow::Result<String> {
    let customers = timada_customer::Command(executor);
    let customer_id = customers
        .register_customer(timada_customer::RegisterCustomer {
            email: email.into(),
            civility: timada_core::Civility::Mrs,
            first_name: "Grete".into(),
            last_name: "Hermann".into(),
        })
        .await?;
    customers
        .identify_company(
            &customer_id,
            "Hermann GmbH",
            &timada_tax::VatNumber::parse(vat_number)?,
        )
        .await?;
    customers
        .check_company_vat_number(&customer_id, registry)
        .await?;

    let cart = timada_cart::Command(executor);
    let cart_id = cart.open_cart(Some(customer_id)).await?;
    cart.add_line(
        &cart_id,
        AddLine {
            product_id: PRODUCT.into(),
            name: "AOC 23.8\" LED - 24G4XE".into(),
            quantity: 1,
            unit_price: Money::eur(12_000),
            warranty_months: 0,
        },
    )
    .await?;
    let (code, method) = country;
    cart.checkout(
        &cart_id,
        Checkout {
            customer_id: None,
            delivery_address: address("Unter den Linden 1", "10117", "Berlin", code),
            billing_address: address("Unter den Linden 1", "10117", "Berlin", code),
            delivery: timada_cart::DeliveryChoice {
                method_code: method.into(),
                pickup_store_id: None,
            },
            payment_mode: timada_cart::PaymentMode::Card,
        },
    )
    .await?;
    Ok(cart_id)
}

#[tokio::test]
async fn a_business_of_another_member_state_buys_without_vat() -> anyhow::Result<()> {
    use timada_tax::{EU_DELIVERY_METHOD, FakeValidator, TaxTreatment, VatNumber, VatRegistry};

    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    timada_pricing::Command(&executor)
        .list_price(timada_pricing::ListPrice {
            product_id: PRODUCT.into(),
            price_incl_tax: Money::eur(12_000),
            vat_rate_bp: 2_000,
            eco_participation: Money::eur(0),
        })
        .await?;
    let registry = FakeValidator::default();
    let place = |policy: timada_order::ReverseChargePolicy| {
        let (executor, db, registry) = (&executor, db.clone(), registry.clone());
        async move {
            order_checkout_subscription()
                .data(db)
                .data(timada_tax::TaxZones::france_with_eu_oss())
                .data(VatRegistry::new(registry))
                .data(policy)
                .run_once(executor)
                .await
        }
    };
    let order_of = |cart_id: String| {
        let executor = &executor;
        async move {
            load_order_details(executor, order_id(&cart_id))
                .await?
                .ok_or_else(|| anyhow::anyhow!("order not placed"))
        }
    };
    let europe = ("DE", EU_DELIVERY_METHOD);

    // A valid German number, delivered in Germany: the pre-tax price, no VAT,
    // and the proof of the check kept on the order.
    let cart =
        business_checkout(&executor, &registry, "a@example.de", "DE123456789", europe).await?;
    place(timada_order::ReverseChargePolicy::default()).await?;
    let order = order_of(cart).await?;
    assert_eq!(order.lines[0].unit_price, Money::eur(10_000));
    // Delivery follows: 12,90 listed = 10,75 without VAT.
    assert_eq!(order.shipping_fee, Money::eur(1_075));
    assert_eq!(order.total, Money::eur(11_075));
    let tax = order
        .tax
        .clone()
        .ok_or_else(|| anyhow::anyhow!("order not taxed"))?;
    assert_eq!(
        (tax.zone_code.as_str(), tax.treatment),
        ("de", TaxTreatment::Export)
    );
    assert_eq!(tax.vat_total()?, Money::eur(0));
    let buyer = order
        .buyer
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no buyer on the order"))?;
    assert_eq!(
        (buyer.company_name.as_str(), buyer.vat_number.as_str()),
        ("Hermann GmbH", "DE123456789")
    );
    let proof = order
        .reverse_charge
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no reverse charge"))?;
    assert!(
        proof
            .consultation_ref
            .is_some_and(|r| r.starts_with("FAKE-DE123456789"))
    );
    assert_eq!(
        order.regime_mention(),
        Some(timada_tax::REVERSE_CHARGE_MENTION)
    );
    // Checked a moment ago at sign-up: not asked again at checkout.
    assert_eq!(registry.checks(), 1);

    // A policy that always asks again; the registry now refuses the number:
    // the order is a consumer's — German VAT — though still a business's.
    let always = timada_order::ReverseChargePolicy {
        recheck_after: std::time::Duration::ZERO,
        ..Default::default()
    };
    let cart =
        business_checkout(&executor, &registry, "b@example.de", "DE999999999", europe).await?;
    registry.reject(&VatNumber::parse("DE999999999")?);
    place(always.clone()).await?;
    let order = order_of(cart).await?;
    assert_eq!(order.lines[0].unit_price, Money::eur(11_900));
    assert!(order.reverse_charge.is_none());
    assert!(order.buyer.is_some());
    assert_eq!(
        order.tax.map(|tax| tax.treatment),
        Some(TaxTreatment::DestinationVat)
    );

    // The registry is down at checkout: the valid answer of a moment ago
    // stands — until it is older than the policy allows.
    let cart =
        business_checkout(&executor, &registry, "c@example.at", "ATU12345678", europe).await?;
    registry.set_down(true);
    place(always.clone()).await?;
    assert!(order_of(cart).await?.reverse_charge.is_some());
    let cart =
        business_checkout(&executor, &registry, "d@example.at", "ATU87654321", europe).await?;
    // (the sign-up check itself found the registry down: never validated)
    place(always.clone()).await?;
    assert!(order_of(cart).await?.reverse_charge.is_none());
    registry.set_down(false);
    let strict = timada_order::ReverseChargePolicy {
        max_check_age: std::time::Duration::ZERO,
        recheck_after: std::time::Duration::from_secs(3_600),
    };
    let cart =
        business_checkout(&executor, &registry, "e@example.at", "ATU11111111", europe).await?;
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    place(strict).await?;
    assert!(
        order_of(cart).await?.reverse_charge.is_none(),
        "too old a check"
    );

    // A business of the shop's own state, or one delivered at home or
    // outside the Union: nothing intra-community about it.
    let cart = business_checkout(
        &executor,
        &registry,
        "f@example.fr",
        "FR40303265045",
        europe,
    )
    .await?;
    place(always.clone()).await?;
    let order = order_of(cart).await?;
    assert!(order.reverse_charge.is_none() && order.buyer.is_some());
    let cart = business_checkout(
        &executor,
        &registry,
        "g@example.de",
        "DE111111111",
        ("FR", "colissimo"),
    )
    .await?;
    place(always).await?;
    let order = order_of(cart).await?;
    assert!(order.reverse_charge.is_none());
    assert_eq!(order.lines[0].unit_price, Money::eur(12_000));
    assert_eq!(
        order.tax.map(|tax| tax.treatment),
        Some(TaxTreatment::Domestic)
    );
    Ok(())
}

#[tokio::test]
async fn a_paid_order_waits_in_the_queue_until_it_ships() -> anyhow::Result<()> {
    use timada_order::{count_orders_to_ship, orders_to_ship};

    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cart_id = checkout_cart(&executor, 5, 1, timada_cart::PaymentMode::Card).await?;
    let order_id = order_id(&cart_id);
    let history = || async {
        drain(&executor, &db).await?;
        order_history_subscription()
            .data(db.clone())
            .run_once(&executor)
            .await
    };
    history().await?;
    let now = timada_core::time::now_unix_secs()?;

    // Placed, not paid: nothing to prepare yet.
    assert!(orders_to_ship(&db, 10, 0).await?.is_empty());
    assert_eq!(count_orders_to_ship(&db, now).await?, (0, 0));

    timada_payment::Command(&executor)
        .capture_payment(payment_id(&order_id), "psp-1".into())
        .await?;
    history().await?;
    let queue = orders_to_ship(&db, 10, 0).await?;
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].order_id, order_id);
    assert!(queue[0].waiting_since >= queue[0].placed_at);
    assert!(queue[0].waiting_since as u64 <= now + 5);
    // Late is whatever has waited since before the moment given.
    assert_eq!(
        count_orders_to_ship(&db, now.saturating_sub(3_600)).await?,
        (1, 0)
    );
    assert_eq!(count_orders_to_ship(&db, now + 3_600).await?, (1, 1));
    // Redelivered: the first date stays.
    let since = queue[0].waiting_since;
    history().await?;
    assert_eq!(orders_to_ship(&db, 10, 0).await?[0].waiting_since, since);

    // The cardholder disputes the charge: the parcel waits for the bank.
    let hold = || async {
        timada_order::payment_hold_subscription()
            .data(db.clone())
            .run_once(&executor)
            .await
    };
    let payments = timada_payment::Command(&executor);
    payments
        .open_dispute(
            payment_id(&order_id),
            timada_payment::OpenDispute {
                dispute_id: "dp_1".into(),
                amount: queue[0].total(),
                reason: "fraudulent".into(),
                respond_by: None,
            },
        )
        .await?;
    hold().await?;
    hold().await?;
    assert!(timada_order::is_order_on_hold(&db, &order_id).await?);
    assert_eq!(timada_order::count_orders_on_hold(&db).await?, 1);
    assert!(orders_to_ship(&db, 10, 0).await?.is_empty());
    assert_eq!(count_orders_to_ship(&db, now + 3_600).await?, (0, 0));
    // Won: back in the queue, where it was.
    payments.win_dispute(payment_id(&order_id), "dp_1").await?;
    hold().await?;
    assert!(!timada_order::is_order_on_hold(&db, &order_id).await?);
    assert_eq!(timada_order::count_orders_on_hold(&db).await?, 0);
    assert_eq!(orders_to_ship(&db, 10, 0).await?[0].waiting_since, since);

    timada_shipping::Command(&executor)
        .dispatch_shipment(shipment_id(&order_id), "Chronopost".into(), "XY123".into())
        .await?;
    history().await?;
    assert!(orders_to_ship(&db, 10, 0).await?.is_empty());
    assert_eq!(count_orders_to_ship(&db, now + 3_600).await?, (0, 0));
    Ok(())
}

#[tokio::test]
async fn delivery_and_instalment_fees_are_charged_in_the_carts_currency() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let fees = timada_shipping::DeliveryFees::default()
        .with_fee("colissimo-europe", Money::new(1_190, "GBP"));
    // Paying in several times costs 3,99 £; the host said nothing of francs.
    let handling = timada_order::InstallmentHandlingFees(
        timada_core::PerCurrency::none()
            .with(Money::eur(449))
            .with(Money::new(399, "GBP")),
    );
    let inventory = timada_inventory::Command(&executor);
    let stock = inventory
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Warehouse,
        })
        .await?;
    inventory.receive_stock(&stock, 10).await?;

    let cart = timada_cart::Command(&executor);
    let mut carts = Vec::new();
    for currency in ["GBP", "CHF"] {
        let cart_id = cart.open_cart(Some(CUSTOMER.into())).await?;
        cart.add_line(
            &cart_id,
            AddLine {
                product_id: PRODUCT.into(),
                name: "AOC 23.8\" LED - 24G4XE".into(),
                quantity: 1,
                unit_price: Money::new(10_000, currency),
                warranty_months: 60,
            },
        )
        .await?;
        cart.checkout(
            &cart_id,
            Checkout {
                customer_id: None,
                delivery_address: address("121, Avenue Tolosane", "31520", "Ramonville", "FR"),
                billing_address: address("121, Avenue Tolosane", "31520", "Ramonville", "FR"),
                delivery: timada_cart::DeliveryChoice {
                    method_code: "colissimo-europe".into(),
                    pickup_store_id: None,
                },
                payment_mode: timada_cart::PaymentMode::Installments { count: 3 },
            },
        )
        .await?;
        carts.push(cart_id);
    }
    order_checkout_subscription()
        .data(db.clone())
        .data(fees)
        .data(handling)
        .run_once(&executor)
        .await?;

    // Pounds: the host's pound fee, and a total in pounds — nothing in euros
    // slipped in.
    let pounds = timada_order::load_order_details(&executor, order_id(&carts[0]))
        .await?
        .ok_or_else(|| anyhow::anyhow!("pound order missing"))?;
    assert_eq!(pounds.shipping_fee, Money::new(1_190, "GBP"));
    assert_eq!(pounds.handling_fee, Money::new(399, "GBP"));
    assert_eq!(pounds.total, Money::new(11_589, "GBP"));
    // Francs: the host priced no delivery there. The storefront would not
    // have offered the method; should it slip by, the order is kept.
    let francs = timada_order::load_order_details(&executor, order_id(&carts[1]))
        .await?
        .ok_or_else(|| anyhow::anyhow!("franc order missing"))?;
    assert_eq!(francs.shipping_fee, Money::new(0, "CHF"));
    assert_eq!(francs.handling_fee, Money::new(0, "CHF"));
    assert_eq!(francs.total, Money::new(10_000, "CHF"));
    Ok(())
}

/// Quotes pounds, and fails for everything else as a source that is down.
struct PoundsOnly;

impl timada_tax::ExchangeRates for PoundsOnly {
    fn rate<'a>(&'a self, base: &'a str, currency: &'a str, at: u64) -> timada_tax::RateFuture<'a> {
        Box::pin(async move {
            if currency == "GBP" {
                Ok(timada_tax::PinnedRate {
                    base: base.to_owned(),
                    currency: currency.to_owned(),
                    per_base_micros: 853_800,
                    as_of: at,
                    source: "BCE".into(),
                })
            } else {
                Err(timada_tax::RateError::Unavailable("bank closed".into()))
            }
        })
    }
}

#[tokio::test]
async fn an_order_in_another_currency_is_pinned_the_rate_of_its_day() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let inventory = timada_inventory::Command(&executor);
    let stock = inventory
        .register_stock_item(RegisterStockItem {
            product_id: PRODUCT.into(),
            location: StockLocation::Warehouse,
        })
        .await?;
    inventory.receive_stock(&stock, 10).await?;

    let cart = timada_cart::Command(&executor);
    let mut carts = Vec::new();
    for currency in ["EUR", "GBP", "CHF"] {
        let cart_id = cart.open_cart(Some(CUSTOMER.into())).await?;
        cart.add_line(
            &cart_id,
            AddLine {
                product_id: PRODUCT.into(),
                name: "AOC 23.8\" LED - 24G4XE".into(),
                quantity: 1,
                unit_price: Money::new(10_000, currency),
                warranty_months: 60,
            },
        )
        .await?;
        cart.checkout(
            &cart_id,
            Checkout {
                customer_id: None,
                delivery_address: address("121, Avenue Tolosane", "31520", "Ramonville", "FR"),
                billing_address: address("121, Avenue Tolosane", "31520", "Ramonville", "FR"),
                delivery: timada_cart::DeliveryChoice {
                    method_code: "store-pickup".into(),
                    pickup_store_id: Some("toulouse".into()),
                },
                payment_mode: timada_cart::PaymentMode::Card,
            },
        )
        .await?;
        carts.push(order_id(&cart_id));
    }
    for _ in 0..2 {
        order_checkout_subscription()
            .data(db.clone())
            .data(timada_core::ShopCurrencies::new("EUR", &["GBP", "CHF"])?)
            .data(timada_tax::ExchangeRateSource::new(PoundsOnly))
            .data(
                timada_shipping::DeliveryFees::default()
                    .with_fee("store-pickup", Money::new(0, "GBP"))
                    .with_fee("store-pickup", Money::new(0, "CHF")),
            )
            .run_once(&executor)
            .await?;
    }
    let rate_of = |id: String| {
        let executor = &executor;
        async move {
            Ok::<_, anyhow::Error>(
                timada_order::load_order_details(executor, id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("order missing"))?
                    .exchange_rate,
            )
        }
    };
    // The books' own currency needs no rate.
    assert_eq!(rate_of(carts[0].clone()).await?, None);
    // Pounds: the rate of the day the cart was checked out.
    let pounds = rate_of(carts[1].clone())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no rate pinned on the pound order"))?;
    assert_eq!(
        (
            pounds.base.as_str(),
            pounds.currency.as_str(),
            pounds.per_base_micros
        ),
        ("EUR", "GBP", 853_800)
    );
    assert!(pounds.as_of > 0);
    // Francs: the source was down. The order is placed all the same, and
    // gets its rate later — once.
    assert_eq!(rate_of(carts[2].clone()).await?, None);
    let late = timada_tax::PinnedRate {
        base: "EUR".into(),
        currency: "CHF".into(),
        per_base_micros: 941_200,
        as_of: pounds.as_of,
        source: "BCE".into(),
    };
    let orders = timada_order::Command(&executor);
    assert!(orders.pin_exchange_rate(&carts[2], late.clone()).await?);
    assert!(!orders.pin_exchange_rate(&carts[2], late.clone()).await?);
    assert_eq!(rate_of(carts[2].clone()).await?, Some(late));
    Ok(())
}
