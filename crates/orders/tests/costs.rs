mod common;

use common::*;
use timada_catalog::{ShopCosts, UnitCosts, VariantCost};
use timada_orders::{OpenedPayment, OrderCosts, OrderLine, PROVIDER_STRIPE, Payment};

fn shop_costs() -> ShopCosts {
    ShopCosts {
        currency: "EUR".into(),
        shipping_cents: 400,
        import_cents: 500,
        fixed_cents: 100,
        payment_fee_bps: 150,
        payment_fee_cents: 25,
    }
}

/// Opens a payment for the cart as the dashboard would, saying the VAT the
/// sale is expected to carry; the payment to place the order with.
async fn open_with_vat(orders: &Orders, cart_id: &str, vat_cents: u64) -> Payment {
    let request_id = orders
        .request_checkout(SHOPPER, cart_id, checkout())
        .await
        .unwrap();
    let mut pending = orders
        .pending_payment(cart_id, &request_id)
        .await
        .unwrap()
        .unwrap();
    if let Some(costs) = &mut pending.costs {
        costs.vat_cents = vat_cents;
    }
    let payment = Payment {
        provider: PROVIDER_STRIPE.into(),
        payment_id: format!("pi_{request_id}"),
        amount_cents: pending.total_cents,
        currency: pending.currency.clone(),
        capture_before: None,
    };
    orders
        .record_payment_request(
            DASHBOARD,
            cart_id,
            pending,
            OpenedPayment {
                provider: PROVIDER_STRIPE.into(),
                payment_id: payment.payment_id.clone(),
                client_secret: "secret".into(),
                publishable_key: "pk_test".into(),
            },
        )
        .await
        .unwrap();
    payment
}

#[tokio::test]
async fn an_order_keeps_what_its_sale_cost_when_it_was_made() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    catalog.set_cost(ALICE, &mug, Some(400)).await.unwrap();
    let (mat, beige, _) = varied_product(&catalog, "Mat").await;
    catalog
        .set_variant_costs(
            ALICE,
            &mat,
            &[VariantCost {
                variant_id: beige.clone(),
                amount_cents: Some(300),
            }],
        )
        .await
        .unwrap();
    // The mat comes from inside the EU, by its supplier's own carrier.
    catalog
        .set_own_costs(ALICE, &mat, Some(250), Some(0))
        .await
        .unwrap();
    catalog.set_sale_costs(ALICE, shop_costs()).await.unwrap();

    let cart_id = cart_with(&orders, &mug, 2).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();
    let payment = open_with_vat(&orders, &cart_id, 583).await;

    // The costs move on before the payment lands: the order keeps what
    // they were.
    catalog.set_cost(ALICE, &mug, Some(900)).await.unwrap();
    catalog
        .set_sale_costs(
            ALICE,
            ShopCosts {
                fixed_cents: 9_000,
                ..shop_costs()
            },
        )
        .await
        .unwrap();
    let placed = orders
        .place_order(DASHBOARD, &cart_id, payment)
        .await
        .unwrap();
    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 3_500);
    let costs = order.costs.clone().expect("the sale's costs");
    assert_eq!(
        costs,
        OrderCosts {
            unit_costs: vec![Some(400), Some(300)],
            // The mug the shop's usual way, the mat its own.
            shipping_cents: 400 + 250,
            // The mug at the border, the mat not.
            import_cents: 500,
            fixed_cents: 100,
            // 1.5 % of 35.00, rounded, and 0.25.
            payment_fee_cents: 53 + 25,
            vat_cents: 583,
        }
    );
    assert_eq!(costs.goods_cents(&order.lines), Some(2 * 400 + 300));
    assert_eq!(
        order.profit_cents(),
        Some(3_500 - 1_100 - 650 - 500 - 100 - 78 - 583)
    );
}

#[tokio::test]
async fn a_shop_that_said_no_cost_keeps_none() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    catalog.set_sale_costs(ALICE, shop_costs()).await.unwrap();
    let cart_id = cart_with(&orders, &mug, 1).await;
    let placed = place(&orders, &cart_id, checkout()).await;
    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(order.costs, None);
    assert_eq!(order.profit_cents(), None);
}

#[tokio::test]
async fn a_line_without_a_cost_leaves_the_profit_unsaid() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let plate = product(&catalog, "Plate", 900, "EUR").await;
    catalog.set_cost(ALICE, &mug, Some(400)).await.unwrap();
    let cart_id = cart_with(&orders, &mug, 1).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&plate, None, 1))
        .await
        .unwrap();
    let placed = place(&orders, &cart_id, checkout()).await;
    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    let costs = order.costs.clone().expect("what is known is kept");
    assert_eq!(costs.unit_costs, vec![Some(400), None]);
    // A shop that said no cost of its own counts none.
    assert_eq!(costs.other_cents(), 0);
    assert_eq!(costs.goods_cents(&order.lines), None);
    assert_eq!(order.profit_cents(), None);
}

fn line(product_id: &str, quantity: u32) -> OrderLine {
    OrderLine {
        product_id: product_id.to_owned(),
        unit_amount_cents: 1_000,
        quantity,
        ..OrderLine::default()
    }
}

#[test]
fn the_border_and_the_carrier_charge_by_product() {
    let unit = |cost_cents, shipping_cents, import_cents| UnitCosts {
        cost_cents,
        shipping_cents,
        import_cents,
    };
    // An offer split the mug in two lines; it is one product at the border.
    let lines = [line("mug", 1), line("mug", 2), line("mat", 1)];
    let costs = OrderCosts::reckon(
        &lines,
        |line| match line.product_id.as_str() {
            "mug" => unit(Some(400), None, None),
            _ => unit(Some(300), None, Some(0)),
        },
        Some(&shop_costs()),
        "EUR",
        4_000,
    )
    .unwrap();
    assert_eq!(costs.unit_costs, vec![Some(400), Some(400), Some(300)]);
    assert_eq!(
        costs.shipping_cents, 400,
        "one parcel, the shop's usual way"
    );
    assert_eq!(costs.import_cents, 500);
    assert_eq!(costs.goods_cents(&lines), Some(3 * 400 + 300));

    // In another currency than the shop's costs, nothing true can be said.
    assert_eq!(
        OrderCosts::reckon(
            &lines,
            |_| unit(Some(400), None, None),
            Some(&shop_costs()),
            "USD",
            4_000,
        ),
        None
    );
    assert_eq!(
        OrderCosts::reckon(&lines, |_| unit(None, Some(1), Some(1)), None, "EUR", 4_000),
        None
    );
}
