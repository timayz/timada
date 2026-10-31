mod common;

use common::*;
use timada_orders::{Checkout, NewShippingMethod, OrderError, split_countries};

fn tracked() -> NewShippingMethod {
    NewShippingMethod {
        name: " Tracked delivery ".to_owned(),
        price_cents: 490,
        currency: "eur".to_owned(),
        countries: split_countries("fr, be FR"),
        free_above_cents: Some(5000),
    }
}

#[tokio::test]
async fn a_shop_ships_however_its_merchant_says() {
    let (orders, _) = store().await;
    assert!(orders.shipping_methods().await.unwrap().is_empty());

    let tracked_id = orders.add_shipping_method(ALICE, tracked()).await.unwrap();
    let vip_id = orders
        .add_shipping_method(
            ALICE,
            NewShippingMethod {
                name: "VIP".to_owned(),
                price_cents: 690,
                currency: "EUR".to_owned(),
                countries: vec![],
                free_above_cents: None,
            },
        )
        .await
        .unwrap();

    let methods = orders.shipping_methods().await.unwrap();
    assert_eq!(methods.len(), 2);
    let tracked = &methods[0];
    assert_eq!(tracked.id, tracked_id);
    assert_eq!(tracked.name, "Tracked delivery");
    assert_eq!(tracked.currency, "EUR");
    assert_eq!(tracked.countries, ["BE", "FR"]);
    assert!(tracked.covers("FR"));
    assert!(!tracked.covers("US"));
    assert!(
        methods[1].covers("US"),
        "no country listed is every country"
    );
    assert_eq!(tracked.price_for(4999), 490);
    assert_eq!(tracked.price_for(5000), 0);
    assert_eq!(methods[1].price_for(1_000_000), 690);

    orders
        .change_shipping_method(
            ALICE,
            &vip_id,
            NewShippingMethod {
                name: "VIP: insured".to_owned(),
                price_cents: 790,
                currency: "EUR".to_owned(),
                countries: vec!["FR".to_owned()],
                free_above_cents: None,
            },
        )
        .await
        .unwrap();
    orders
        .remove_shipping_method(ALICE, &tracked_id)
        .await
        .unwrap();
    let methods = orders.shipping_methods().await.unwrap();
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].name, "VIP: insured");
    assert_eq!(methods[0].price_cents, 790);
    assert!(!methods[0].covers("US"));

    let err = orders
        .remove_shipping_method(ALICE, &tracked_id)
        .await
        .unwrap_err();
    assert!(matches!(err, OrderError::ShippingMethodNotFound));
}

#[tokio::test]
async fn a_shipping_method_is_checked() {
    let (orders, _) = store().await;
    let refused = |method: NewShippingMethod| {
        let orders = &orders;
        async move {
            orders
                .add_shipping_method(ALICE, method)
                .await
                .unwrap_err()
                .code()
        }
    };
    assert_eq!(
        refused(NewShippingMethod {
            name: " ".to_owned(),
            ..tracked()
        })
        .await,
        Some("invalid_shipping_name")
    );
    assert_eq!(
        refused(NewShippingMethod {
            currency: "XYZ".to_owned(),
            ..tracked()
        })
        .await,
        Some("invalid_currency")
    );
    assert_eq!(
        refused(NewShippingMethod {
            countries: vec!["France".to_owned()],
            ..tracked()
        })
        .await,
        Some("invalid_shipping_countries")
    );
    assert_eq!(
        refused(NewShippingMethod {
            price_cents: u64::MAX,
            ..tracked()
        })
        .await,
        Some("invalid_amount")
    );
    assert!(orders.shipping_methods().await.unwrap().is_empty());
}

/// A shop shipping "Tracked" to FR and BE (4.90, free from 50.00) and
/// "VIP" everywhere (6.90); returns their ids.
async fn two_methods(orders: &Orders) -> (String, String) {
    let tracked = orders.add_shipping_method(ALICE, tracked()).await.unwrap();
    let vip = orders
        .add_shipping_method(
            ALICE,
            NewShippingMethod {
                name: "VIP".to_owned(),
                price_cents: 690,
                currency: "EUR".to_owned(),
                countries: vec![],
                free_above_cents: None,
            },
        )
        .await
        .unwrap();
    (tracked, vip)
}

fn shipped_by(method_id: &str) -> Checkout {
    Checkout {
        shipping_method: Some(method_id.to_owned()),
        ..checkout()
    }
}

#[tokio::test]
async fn the_shipping_picked_is_charged_on_top_of_the_goods() {
    let (orders, catalog) = store().await;
    let (_, vip) = two_methods(&orders).await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 2).await;

    let order = pay(
        &orders,
        &cart_id,
        Checkout {
            expected_total_cents: Some(2500 + 690),
            ..shipped_by(&vip)
        },
    )
    .await;
    let order = orders.load_order(&order.order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 3190);
    assert_eq!(order.payment.amount_cents, 3190);
    let charge = order.shipping_charge.expect("shipping charged");
    assert_eq!(charge.method_id, vip);
    assert_eq!(charge.name, "VIP");
    assert_eq!(charge.amount_cents, 690);
}

#[tokio::test]
async fn shipping_is_free_once_the_discounted_goods_reach_the_threshold() {
    let (orders, catalog) = store().await;
    let (tracked, _) = two_methods(&orders).await;
    let lamp = product(&catalog, "Lamp", 5000, "EUR").await;
    let cart_id = cart_with(&orders, &lamp, 1).await;

    let request_id = orders
        .request_checkout(SHOPPER, &cart_id, shipped_by(&tracked))
        .await
        .unwrap();
    let pending = orders
        .pending_payment(&cart_id, &request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.total_cents, 5000);
    assert_eq!(pending.shipping.unwrap().amount_cents, 0);

    // 10 % off brings the goods under 50.00: shipping is charged again.
    timada_discounts::Command::new(orders.executor().clone(), TENANT)
        .create_discount(
            ALICE,
            timada_discounts::CreateDiscount {
                code: "TEN".into(),
                value: timada_discounts::DiscountValue::Percent(1000),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    orders
        .apply_discount(SHOPPER, &cart_id, "TEN")
        .await
        .unwrap();
    let request_id = orders
        .request_checkout(SHOPPER, &cart_id, shipped_by(&tracked))
        .await
        .unwrap();
    let pending = orders
        .pending_payment(&cart_id, &request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.total_cents, 4500 + 490);
    assert_eq!(pending.shipping.unwrap().amount_cents, 490);
}

#[tokio::test]
async fn a_shop_with_methods_refuses_a_checkout_without_one_that_ships_there() {
    let (orders, catalog) = store().await;
    let (tracked, vip) = two_methods(&orders).await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let refused = async |checkout: Checkout| {
        let request_id = orders
            .request_checkout(SHOPPER, &cart_id, checkout)
            .await
            .unwrap();
        orders
            .pending_payment(&cart_id, &request_id)
            .await
            .unwrap_err()
            .code()
    };

    // None picked.
    assert_eq!(refused(checkout()).await, Some("shipping_unavailable"));
    // Tracked does not ship to the US.
    let mut to_us = shipped_by(&tracked);
    to_us.shipping.country = "US".to_owned();
    assert_eq!(refused(to_us).await, Some("shipping_unavailable"));
    // A method since removed.
    orders.remove_shipping_method(ALICE, &vip).await.unwrap();
    assert_eq!(
        refused(shipped_by(&vip)).await,
        Some("shipping_unavailable")
    );
    // Priced in another currency than the goods.
    let dollars = product(&catalog, "Cap", 1000, "USD").await;
    let usd_cart = cart_with(&orders, &dollars, 1).await;
    let request_id = orders
        .request_checkout(SHOPPER, &usd_cart, shipped_by(&tracked))
        .await
        .unwrap();
    let err = orders
        .pending_payment(&usd_cart, &request_id)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("shipping_unavailable"));
}

#[tokio::test]
async fn a_shop_without_methods_ships_for_free() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    // A method named anyway is ignored.
    let order = pay(&orders, &cart_id, shipped_by("gone")).await;
    let order = orders.load_order(&order.order_id).await.unwrap().unwrap();
    assert_eq!(order.total_cents, 1250);
    assert_eq!(order.shipping_charge, None);
}
