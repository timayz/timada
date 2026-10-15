mod common;

use common::*;
use timada_orders::{
    Checkout, OrderStatus, OrdersError, Payment, PaymentAnswer, ShippingAddress, order_id,
};

#[tokio::test]
async fn a_paid_order_holds_what_the_payment_was_asked_for() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let (mat, beige, _) = varied_product(&catalog, "Mat").await;
    let cart_id = cart_with(&orders, &mug, 2).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();

    let (request_id, payment) = open_payment(
        &orders,
        &cart_id,
        Checkout {
            note: Some("  Leave it at the door ".into()),
            expected_total_cents: Some(3500),
            vat_number: None,
            ..checkout()
        },
    )
    .await;
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert!(cart.is_paying());
    assert_eq!(cart.status(), "paying");
    let snapshot = cart
        .request(&request_id)
        .unwrap()
        .payment()
        .unwrap()
        .clone();
    assert_eq!(snapshot.total_cents, 3500);
    assert_eq!(
        snapshot.client_secret,
        format!("{}_secret", payment.payment_id)
    );

    // The catalog moves on before the payment lands: the order holds what
    // was paid for.
    catalog
        .rename_product(ALICE, &mug, "Big mug")
        .await
        .unwrap();
    catalog
        .price_product(ALICE, &mug, 9999, "EUR")
        .await
        .unwrap();
    assert!(orders.load_order_of_cart(&cart_id).await.unwrap().is_none());
    let placed = orders
        .place_paid_order(DASHBOARD, &cart_id, payment.clone())
        .await
        .unwrap();
    assert_eq!(placed.order_id, order_id(&cart_id));

    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(order.payment, payment);
    assert_eq!(order.total_cents, 2 * 1250 + 1000);
    assert_eq!(order.note.as_deref(), Some("Leave it at the door"));
    assert_eq!(order.email, "ada@example.org");
    assert_eq!(order.lines[0].name, "Mug");
    assert_eq!(order.lines[1].variant_label.as_deref(), Some("Beige"));
    assert_eq!(order.lines[1].sku.as_deref(), Some("MAT-B"));
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(cart.order_id.as_deref(), Some(placed.order_id.as_str()));
    assert_eq!(cart.status(), "ordered");
    let by_reference = orders
        .load_order_by_reference(&placed.reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_reference.id, placed.order_id);
}

#[tokio::test]
async fn the_same_payment_told_twice_is_one_order() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let (_, payment) = open_payment(&orders, &cart_id, checkout()).await;

    let first = orders
        .place_paid_order(DASHBOARD, &cart_id, payment.clone())
        .await
        .unwrap();
    let again = orders
        .place_paid_order(DASHBOARD, &cart_id, payment)
        .await
        .unwrap();
    assert_eq!(first, again);
}

#[tokio::test]
async fn a_second_payment_of_an_ordered_cart_is_a_duplicate() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    // Two tabs, two payments opened before either was paid.
    let (_, first) = open_payment(&orders, &cart_id, checkout()).await;
    let (_, second) = open_payment(&orders, &cart_id, checkout()).await;

    let placed = orders
        .place_paid_order(DASHBOARD, &cart_id, first)
        .await
        .unwrap();
    let duplicate = orders.place_paid_order(DASHBOARD, &cart_id, second).await;
    assert!(
        matches!(&duplicate, Err(OrdersError::DuplicatePayment { order_id }) if *order_id == placed.order_id),
        "{duplicate:?}"
    );
}

#[tokio::test]
async fn a_payment_must_match_what_was_asked_for() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let (_, payment) = open_payment(&orders, &cart_id, checkout()).await;

    let short = orders
        .place_paid_order(
            DASHBOARD,
            &cart_id,
            Payment {
                amount_cents: 1,
                ..payment.clone()
            },
        )
        .await;
    assert!(
        matches!(short, Err(OrdersError::AmountMismatch)),
        "{short:?}"
    );
    let unknown = orders
        .place_paid_order(
            DASHBOARD,
            &cart_id,
            Payment {
                payment_id: "pi_other".into(),
                ..payment
            },
        )
        .await;
    assert!(
        matches!(unknown, Err(OrdersError::PaymentNotFound)),
        "{unknown:?}"
    );
    assert!(orders.load_order_of_cart(&cart_id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_request_is_priced_once_and_refused_when_it_cannot_go_ahead() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 2).await;

    // The price moved since the shopper saw 25.00.
    catalog
        .price_product(ALICE, &mug, 1300, "EUR")
        .await
        .unwrap();
    let request_id = orders
        .request_checkout(
            SHOPPER,
            &cart_id,
            Checkout {
                expected_total_cents: Some(2500),
                vat_number: None,
                ..checkout()
            },
        )
        .await
        .unwrap();
    let pending = orders.pending_payment(&cart_id, &request_id).await;
    assert!(
        matches!(
            pending,
            Err(OrdersError::PriceChanged { total_cents: 2600 })
        ),
        "{pending:?}"
    );
    let code = pending.unwrap_err().code().unwrap();
    orders
        .refuse_payment_request(DASHBOARD, &cart_id, &request_id, code)
        .await
        .unwrap();
    // Answered: nothing more to do, and a second answer changes nothing.
    assert!(
        orders
            .pending_payment(&cart_id, &request_id)
            .await
            .unwrap()
            .is_none()
    );
    orders
        .refuse_payment_request(DASHBOARD, &cart_id, &request_id, "other")
        .await
        .unwrap();
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.request(&request_id).unwrap().answer,
        Some(PaymentAnswer::Refused("price_changed".into()))
    );
    assert_eq!(cart.status(), "open");

    // Off sale.
    catalog.unpublish_product(ALICE, &mug).await.unwrap();
    let request_id = orders
        .request_checkout(SHOPPER, &cart_id, checkout())
        .await
        .unwrap();
    let pending = orders.pending_payment(&cart_id, &request_id).await;
    assert!(
        matches!(pending, Err(OrdersError::Unavailable { .. })),
        "{pending:?}"
    );
}

#[tokio::test]
async fn an_empty_cart_or_a_bad_address_cannot_ask_to_pay() {
    let (orders, catalog) = store().await;
    let empty = orders.create_cart(SHOPPER).await.unwrap();
    let refused = orders.request_checkout(SHOPPER, &empty, checkout()).await;
    assert!(
        matches!(refused, Err(OrdersError::EmptyCart)),
        "{refused:?}"
    );

    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let refused = orders
        .request_checkout(
            SHOPPER,
            &cart_id,
            Checkout {
                email: "not an address".into(),
                ..checkout()
            },
        )
        .await;
    assert!(
        matches!(refused, Err(OrdersError::InvalidEmail)),
        "{refused:?}"
    );
    let refused = orders
        .request_checkout(
            SHOPPER,
            &cart_id,
            Checkout {
                shipping: ShippingAddress {
                    postal_code: " ".into(),
                    ..checkout().shipping
                },
                ..checkout()
            },
        )
        .await;
    assert!(
        matches!(refused, Err(OrdersError::InvalidAddress)),
        "{refused:?}"
    );
    let refused = orders
        .request_checkout(SHOPPER, "no-such-cart", checkout())
        .await;
    assert!(
        matches!(refused, Err(OrdersError::CartNotFound)),
        "{refused:?}"
    );
}

#[tokio::test]
async fn every_order_has_its_own_reference() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let mut references = std::collections::BTreeSet::new();
    for _ in 0..10 {
        let cart_id = cart_with(&orders, &mug, 1).await;
        references.insert(pay(&orders, &cart_id, checkout()).await.reference);
    }
    assert_eq!(references.len(), 10);
}

#[tokio::test]
async fn a_business_buyer_gives_its_vat_number_with_the_request() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart = cart_with(&orders, &mug, 1).await;
    orders.set_business_buyers(ALICE, true).await.unwrap();

    let err = orders
        .request_checkout(
            SHOPPER,
            &cart,
            Checkout {
                vat_number: Some("US123".into()),
                ..checkout()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("invalid_vat_number"));

    let request_id = orders
        .request_checkout(
            SHOPPER,
            &cart,
            Checkout {
                vat_number: Some(" de 123.456.789 ".into()),
                ..checkout()
            },
        )
        .await
        .unwrap();
    let state = orders.load_cart(&cart).await.unwrap().unwrap();
    let request = state.request(&request_id).unwrap();
    assert_eq!(request.vat_number.as_deref(), Some("DE123456789"));

    // A blank number is no number.
    let plain = orders
        .request_checkout(
            SHOPPER,
            &cart,
            Checkout {
                vat_number: Some("  ".into()),
                ..checkout()
            },
        )
        .await
        .unwrap();
    let state = orders.load_cart(&cart).await.unwrap().unwrap();
    assert_eq!(state.request(&plain).unwrap().vat_number, None);
    assert!(timada_orders::is_shopper_event(
        "timada-orders/Cart",
        "BuyerIdentified"
    ));
}

#[tokio::test]
async fn a_shop_sells_to_businesses_only_once_its_merchant_says_so() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart = cart_with(&orders, &mug, 1).await;
    let as_business = || Checkout {
        vat_number: Some("DE123456789".into()),
        ..checkout()
    };

    assert!(!orders.accepts_business_buyers().await.unwrap());
    let err = orders
        .request_checkout(SHOPPER, &cart, as_business())
        .await
        .unwrap_err();
    assert!(matches!(err, OrdersError::BusinessBuyersRefused));
    assert_eq!(err.code(), Some("business_buyers_refused"));
    // A blank number is still a consumer's checkout.
    orders
        .request_checkout(
            SHOPPER,
            &cart,
            Checkout {
                vat_number: Some("  ".into()),
                ..checkout()
            },
        )
        .await
        .unwrap();

    orders.set_business_buyers(ALICE, true).await.unwrap();
    assert!(orders.accepts_business_buyers().await.unwrap());
    orders
        .request_checkout(SHOPPER, &cart, as_business())
        .await
        .unwrap();

    // Saying it twice is fine; closing it again refuses again.
    orders.set_business_buyers(ALICE, true).await.unwrap();
    orders.set_business_buyers(ALICE, false).await.unwrap();
    let err = orders
        .request_checkout(SHOPPER, &cart, as_business())
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("business_buyers_refused"));
}
