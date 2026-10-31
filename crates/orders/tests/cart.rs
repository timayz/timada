mod common;

use common::*;
use timada_orders::{MAX_CART_LINES, MAX_LINE_QUANTITY, OrderError};

#[tokio::test]
async fn adding_twice_adds_to_the_same_line() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();

    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 2))
        .await
        .unwrap();
    let cart = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 3))
        .await
        .unwrap();

    assert_eq!(cart.lines.len(), 1);
    assert_eq!(cart.lines[0].quantity, 5);
    assert_eq!(cart.item_count(), 5);
}

#[tokio::test]
async fn variants_are_required_exactly_when_the_product_sells_them() {
    let (orders, catalog) = store().await;
    let (mat, beige, blue) = varied_product(&catalog, "Mat").await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();

    let missing = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, None, 1))
        .await;
    assert!(
        matches!(missing, Err(OrderError::VariantRequired)),
        "{missing:?}"
    );
    let unknown = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some("nope"), 1))
        .await;
    assert!(
        matches!(unknown, Err(OrderError::VariantNotFound)),
        "{unknown:?}"
    );
    let extra = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, Some(&beige), 1))
        .await;
    assert!(
        matches!(extra, Err(OrderError::VariantNotFound)),
        "{extra:?}"
    );

    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();
    let cart = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&blue), 2))
        .await
        .unwrap();
    assert_eq!(cart.lines.len(), 2, "one line per variant");
}

#[tokio::test]
async fn only_products_on_sale_go_in() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    catalog.unpublish_product(ALICE, &mug).await.unwrap();
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();

    let draft = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 1))
        .await;
    assert!(
        matches!(draft, Err(OrderError::Unavailable { .. })),
        "{draft:?}"
    );
    let missing = orders
        .add_to_cart(SHOPPER, &cart_id, item("no-such-product", None, 1))
        .await;
    assert!(
        matches!(missing, Err(OrderError::Unavailable { .. })),
        "{missing:?}"
    );
}

#[tokio::test]
async fn quantities_and_lines_are_bounded() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();

    for quantity in [0, MAX_LINE_QUANTITY + 1] {
        let refused = orders
            .add_to_cart(SHOPPER, &cart_id, item(&mug, None, quantity))
            .await;
        assert!(
            matches!(refused, Err(OrderError::InvalidQuantity)),
            "{refused:?}"
        );
    }
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, MAX_LINE_QUANTITY))
        .await
        .unwrap();
    let past = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 1))
        .await;
    assert!(matches!(past, Err(OrderError::InvalidQuantity)), "{past:?}");

    let full = orders.create_cart(SHOPPER).await.unwrap();
    for n in 0..MAX_CART_LINES {
        let id = product(&catalog, &format!("Item {n}"), 100, "EUR").await;
        orders
            .add_to_cart(SHOPPER, &full, item(&id, None, 1))
            .await
            .unwrap();
    }
    let one_more = orders
        .add_to_cart(SHOPPER, &full, item(&mug, None, 1))
        .await;
    assert!(
        matches!(one_more, Err(OrderError::TooManyLines)),
        "{one_more:?}"
    );
}

#[tokio::test]
async fn one_cart_one_currency() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cup = product(&catalog, "Cup", 900, "USD").await;
    let cart_id = cart_with(&orders, &mug, 1).await;

    let mixed = orders
        .add_to_cart(SHOPPER, &cart_id, item(&cup, None, 1))
        .await;
    assert!(
        matches!(mixed, Err(OrderError::MixedCurrencies)),
        "{mixed:?}"
    );
}

#[tokio::test]
async fn quantities_change_and_lines_go() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 2).await;

    let cart = orders
        .set_cart_quantity(SHOPPER, &cart_id, item(&mug, None, 7))
        .await
        .unwrap();
    assert_eq!(cart.lines[0].quantity, 7);
    let cart = orders
        .set_cart_quantity(SHOPPER, &cart_id, item(&mug, None, 0))
        .await
        .unwrap();
    assert!(cart.lines.is_empty());

    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 1))
        .await
        .unwrap();
    let cart = orders
        .remove_from_cart(SHOPPER, &cart_id, &mug, None)
        .await
        .unwrap();
    assert!(cart.lines.is_empty());
    // Not there any more: a no-op.
    orders
        .remove_from_cart(SHOPPER, &cart_id, &mug, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_checked_out_cart_takes_no_more_changes() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    pay(&orders, &cart_id, checkout()).await;

    let added = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 1))
        .await;
    assert!(matches!(added, Err(OrderError::CheckedOut)), "{added:?}");
    let removed = orders.remove_from_cart(SHOPPER, &cart_id, &mug, None).await;
    assert!(
        matches!(removed, Err(OrderError::CheckedOut)),
        "{removed:?}"
    );
    let unknown = orders
        .add_to_cart(SHOPPER, "no-such-cart", item(&mug, None, 1))
        .await;
    assert!(
        matches!(unknown, Err(OrderError::CartNotFound)),
        "{unknown:?}"
    );
}
