mod common;

use common::*;
use timada_orders::{MAX_CART_LINES, MAX_LINE_QUANTITY, OrderError, read_model_subscription};

#[tokio::test]
async fn adding_twice_adds_to_the_same_line() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 2).await;

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
    let cart_id = orders
        .start_cart(SHOPPER, item(&mat, Some(&beige), 1))
        .await
        .unwrap();

    // Refused alike when the item would start a cart and when it goes in
    // one.
    for (wrong, missing_variant) in [
        (item(&mat, None, 1), true),
        (item(&mat, Some("nope"), 1), false),
        (item(&mug, Some(&beige), 1), false),
    ] {
        let started = orders.start_cart(SHOPPER, wrong.clone()).await;
        let added = orders.add_to_cart(SHOPPER, &cart_id, wrong).await;
        for refused in [started.map(|_| ()), added.map(|_| ())] {
            if missing_variant {
                assert!(
                    matches!(refused, Err(OrderError::VariantRequired)),
                    "{refused:?}"
                );
            } else {
                assert!(
                    matches!(refused, Err(OrderError::VariantNotFound)),
                    "{refused:?}"
                );
            }
        }
    }

    let cart = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&blue), 2))
        .await
        .unwrap();
    assert_eq!(cart.lines.len(), 2, "one line per variant");
}

#[tokio::test]
async fn a_variant_a_hidden_option_leaves_off_sale_cannot_be_bought() {
    let (orders, catalog) = store().await;
    let (mat, beige, blue) = varied_product(&catalog, "Mat").await;
    let colour = catalog.load_product(&mat).await.unwrap().unwrap().options[0]
        .id
        .clone();
    let cart_id = orders
        .start_cart(SHOPPER, item(&mat, Some(&blue), 1))
        .await
        .unwrap();

    catalog
        .hide_option(ALICE, &mat, &colour, "Beige")
        .await
        .unwrap();

    // Blue, already in the cart, no longer sells; nor can more of it go in.
    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert!(quote.lines.is_empty());
    assert_eq!(quote.unavailable.len(), 1);
    let refused = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&blue), 1))
        .await;
    assert!(
        matches!(refused, Err(OrderError::VariantNotFound)),
        "{refused:?}"
    );

    // Beige does, with nothing to read: the shopper chose no value.
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();
    let quote = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(quote.lines.len(), 1);
    assert_eq!(quote.lines[0].variant_id.as_deref(), Some(beige.as_str()));
    assert_eq!(quote.lines[0].unit_amount_cents, 1000);
    assert_eq!(quote.lines[0].variant_label, None);
}

#[tokio::test]
async fn only_products_on_sale_go_in() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cup = product(&catalog, "Cup", 900, "EUR").await;
    let cart_id = cart_with(&orders, &cup, 1).await;
    catalog.unpublish_product(ALICE, &mug).await.unwrap();

    for wrong in [item(&mug, None, 1), item("no-such-product", None, 1)] {
        let started = orders.start_cart(SHOPPER, wrong.clone()).await;
        assert!(
            matches!(started, Err(OrderError::Unavailable { .. })),
            "{started:?}"
        );
        let added = orders.add_to_cart(SHOPPER, &cart_id, wrong).await;
        assert!(
            matches!(added, Err(OrderError::Unavailable { .. })),
            "{added:?}"
        );
    }
}

#[tokio::test]
async fn an_item_refused_starts_no_cart() {
    let (executor, pool) = executor_with_pool().await;
    let orders = timada_orders::Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    catalog.set_stock(ALICE, &mug, None, 1).await.unwrap();

    for wrong in [
        item(&mug, None, 0),
        item(&mug, None, 2),
        item(&mug, Some("nope"), 1),
        item("no-such-product", None, 1),
    ] {
        assert!(orders.start_cart(SHOPPER, wrong).await.is_err());
    }
    let started = cart_with(&orders, &mug, 1).await;

    // The refusals wrote nothing: one cart in the store, whole from its
    // first event.
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    let carts: Vec<(String, i64)> = pool
        .fetch_all("SELECT id, item_count FROM orders_carts", ())
        .await
        .unwrap();
    assert_eq!(carts, vec![(started, 1)]);
}

#[tokio::test]
async fn quantities_and_lines_are_bounded() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cup = product(&catalog, "Cup", 900, "EUR").await;
    let cart_id = cart_with(&orders, &cup, 1).await;

    for quantity in [0, MAX_LINE_QUANTITY + 1] {
        let started = orders.start_cart(SHOPPER, item(&mug, None, quantity)).await;
        assert!(
            matches!(started, Err(OrderError::InvalidQuantity)),
            "{started:?}"
        );
        let added = orders
            .add_to_cart(SHOPPER, &cart_id, item(&mug, None, quantity))
            .await;
        assert!(
            matches!(added, Err(OrderError::InvalidQuantity)),
            "{added:?}"
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

    let first = product(&catalog, "Item 0", 100, "EUR").await;
    let full = cart_with(&orders, &first, 1).await;
    for n in 1..MAX_CART_LINES {
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

/// How long these tests keep an idle cart: thirty days.
const IDLE: u64 = 30 * 24 * 60 * 60;

/// A moment long after anything these tests write.
const MUCH_LATER: u64 = u64::MAX / 2;

#[tokio::test]
async fn a_cart_left_idle_is_dropped_and_gone_for_good() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let touched = orders
        .load_cart(&cart_id)
        .await
        .unwrap()
        .unwrap()
        .updated_at;

    // A second too early, it stays.
    let early = orders
        .expire_cart(DASHBOARD, &cart_id, touched + IDLE - 1, IDLE)
        .await
        .unwrap();
    assert!(!early);
    assert!(orders.load_cart(&cart_id).await.unwrap().is_some());

    let dropped = orders
        .expire_cart(DASHBOARD, &cart_id, touched + IDLE, IDLE)
        .await
        .unwrap();
    assert!(dropped);
    // Gone for every reader and every command: a storefront still naming it
    // is told there is no such cart, and starts another.
    assert!(orders.load_cart(&cart_id).await.unwrap().is_none());
    let added = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 1))
        .await;
    assert!(matches!(added, Err(OrderError::CartNotFound)), "{added:?}");
    let asked = orders.request_checkout(SHOPPER, &cart_id, checkout()).await;
    assert!(matches!(asked, Err(OrderError::CartNotFound)), "{asked:?}");
    // Dropped once; an id no cart has is not dropped either.
    for cart in [cart_id.as_str(), "no-such-cart"] {
        let again = orders
            .expire_cart(DASHBOARD, cart, MUCH_LATER, IDLE)
            .await
            .unwrap();
        assert!(!again, "{cart}");
    }
}

#[tokio::test]
async fn a_cart_that_may_still_move_money_is_kept_however_idle() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    // A request to pay nobody answered yet.
    let asked = cart_with(&orders, &mug, 1).await;
    orders
        .request_checkout(SHOPPER, &asked, checkout())
        .await
        .unwrap();
    // A payment open: the shopper may still confirm it.
    let paying = cart_with(&orders, &mug, 1).await;
    let (_, opened) = open_payment(&orders, &paying, checkout()).await;
    // A payment on its way back to the shopper.
    let returning = cart_with(&orders, &mug, 1).await;
    let (_, given_back) = open_payment(&orders, &returning, checkout()).await;
    orders
        .request_payment_return(
            DASHBOARD,
            &returning,
            &given_back.payment_id,
            "out_of_stock",
        )
        .await
        .unwrap();
    // An order.
    let ordered = cart_with(&orders, &mug, 1).await;
    place(&orders, &ordered, checkout()).await;

    for cart_id in [&asked, &paying, &returning, &ordered] {
        let dropped = orders
            .expire_cart(DASHBOARD, cart_id, MUCH_LATER, IDLE)
            .await
            .unwrap();
        assert!(!dropped, "{cart_id}");
        assert!(orders.load_cart(cart_id).await.unwrap().is_some());
    }

    // Once its payment is cancelled, nothing about it can move money.
    orders
        .cancel_payment(DASHBOARD, &paying, &opened.payment_id)
        .await
        .unwrap();
    let dropped = orders
        .expire_cart(DASHBOARD, &paying, MUCH_LATER, IDLE)
        .await
        .unwrap();
    assert!(dropped);
}
