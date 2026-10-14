mod common;

use common::*;
use timada_orders::{Checkout, OrderStatus, OrdersError, ShippingAddress, order_id};

#[tokio::test]
async fn an_order_copies_what_was_sold() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let (mat, beige, _) = varied_product(&catalog, "Mat").await;
    let cart_id = cart_with(&orders, &mug, 2).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();

    let placed = orders
        .place_order(
            SHOPPER,
            &cart_id,
            Checkout {
                note: Some("  Leave it at the door ".into()),
                expected_total_cents: Some(3500),
                ..checkout()
            },
        )
        .await
        .unwrap();
    assert_eq!(placed.order_id, order_id(&cart_id));

    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(order.reference, placed.reference);
    assert_eq!(order.status, OrderStatus::AwaitingPayment);
    assert_eq!(order.currency, "EUR");
    assert_eq!(order.total_cents, 2 * 1250 + 1000);
    assert_eq!(order.item_count(), 3);
    assert_eq!(order.note.as_deref(), Some("Leave it at the door"));
    assert_eq!(order.lines[0].name, "Mug");
    assert_eq!(order.lines[0].variant_label, None);
    assert_eq!(order.lines[1].variant_label.as_deref(), Some("Beige"));
    assert_eq!(order.lines[1].sku.as_deref(), Some("MAT-B"));
    assert_eq!(order.lines[1].unit_amount_cents, 1000);

    // The catalog moving on does not touch the order.
    catalog
        .rename_product(ALICE, &mug, "Big mug")
        .await
        .unwrap();
    catalog
        .price_product(ALICE, &mug, 9999, "EUR")
        .await
        .unwrap();
    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(order.lines[0].name, "Mug");
    assert_eq!(order.total_cents, 3500);

    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(cart.order_id.as_deref(), Some(placed.order_id.as_str()));
    let by_reference = orders
        .load_order_by_reference(&placed.reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_reference.id, placed.order_id);
}

#[tokio::test]
async fn placing_again_returns_the_same_order() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;

    let first = orders
        .place_order(SHOPPER, &cart_id, checkout())
        .await
        .unwrap();
    let again = orders
        .place_order(SHOPPER, &cart_id, checkout())
        .await
        .unwrap();
    assert_eq!(first, again);
}

#[tokio::test]
async fn every_order_has_its_own_reference() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let mut references = std::collections::BTreeSet::new();
    for _ in 0..10 {
        let cart_id = cart_with(&orders, &mug, 1).await;
        let placed = orders
            .place_order(SHOPPER, &cart_id, checkout())
            .await
            .unwrap();
        references.insert(placed.reference);
    }
    assert_eq!(references.len(), 10);
}

#[tokio::test]
async fn a_moved_price_is_never_charged_unseen() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 2).await;
    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.total_cents, 2500);

    catalog
        .price_product(ALICE, &mug, 1300, "EUR")
        .await
        .unwrap();
    let refused = orders
        .place_order(
            SHOPPER,
            &cart_id,
            Checkout {
                expected_total_cents: Some(quote.total_cents),
                ..checkout()
            },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(OrdersError::PriceChanged { total_cents: 2600 })
        ),
        "{refused:?}"
    );
    assert!(orders.load_order_of_cart(&cart_id).await.unwrap().is_none());
}

#[tokio::test]
async fn what_is_off_sale_or_missing_stops_the_order() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let (mat, _, blue) = varied_product(&catalog, "Mat").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&blue), 1))
        .await
        .unwrap();

    catalog.remove_variant(ALICE, &mat, &blue).await.unwrap();
    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.lines.len(), 1);
    assert_eq!(quote.unavailable.len(), 1);
    let refused = orders.place_order(SHOPPER, &cart_id, checkout()).await;
    assert!(
        matches!(&refused, Err(OrdersError::Unavailable { product_id }) if *product_id == mat),
        "{refused:?}"
    );

    orders
        .remove_from_cart(SHOPPER, &cart_id, &mat, Some(&blue))
        .await
        .unwrap();
    catalog.unpublish_product(ALICE, &mug).await.unwrap();
    let refused = orders.place_order(SHOPPER, &cart_id, checkout()).await;
    assert!(
        matches!(refused, Err(OrdersError::Unavailable { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn an_empty_cart_or_a_bad_address_is_refused() {
    let (orders, catalog) = store().await;
    let empty = orders.create_cart(SHOPPER).await.unwrap();
    let refused = orders.place_order(SHOPPER, &empty, checkout()).await;
    assert!(
        matches!(refused, Err(OrdersError::EmptyCart)),
        "{refused:?}"
    );

    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let refused = orders
        .place_order(
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
        .place_order(
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
        .place_order(SHOPPER, "no-such-cart", checkout())
        .await;
    assert!(
        matches!(refused, Err(OrdersError::CartNotFound)),
        "{refused:?}"
    );
}
