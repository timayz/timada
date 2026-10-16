mod common;

use common::*;
use timada_orders::{NewShippingMethod, OrdersError, split_countries};

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
    assert!(matches!(err, OrdersError::ShippingMethodNotFound));
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
