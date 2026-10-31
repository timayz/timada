mod common;

use common::*;
use timada_orders::{OrderError, stock_usage_subscription};

#[tokio::test]
async fn a_cart_takes_no_more_than_is_left() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    catalog.set_stock(ALICE, &mug, None, 2).await.unwrap();

    let cart_id = cart_with(&orders, &mug, 2).await;
    let err = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 1))
        .await
        .unwrap_err();
    assert!(matches!(err, OrderError::OutOfStock { available: 2, .. }));
    assert_eq!(err.code(), Some("out_of_stock"));

    // Fewer is fine; more than is left is not.
    orders
        .set_cart_quantity(SHOPPER, &cart_id, item(&mug, None, 1))
        .await
        .unwrap();
    assert!(matches!(
        orders
            .set_cart_quantity(SHOPPER, &cart_id, item(&mug, None, 3))
            .await,
        Err(OrderError::OutOfStock { .. })
    ));
}

#[tokio::test]
async fn what_another_checkout_holds_is_not_for_sale() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    catalog.set_stock(ALICE, &mug, None, 1).await.unwrap();
    let first = cart_with(&orders, &mug, 1).await;
    let second = cart_with(&orders, &mug, 1).await;
    catalog
        .reserve_stock(
            DASHBOARD,
            &first,
            &[timada_catalog::StockLine {
                product_id: mug.clone(),
                variant_id: None,
                quantity: 1,
            }],
            1800,
        )
        .await
        .unwrap();

    // The first cart still prices; the second shows the line unavailable
    // and its payment is refused for it.
    assert!(
        orders
            .quote_cart(&first)
            .await
            .unwrap()
            .unavailable
            .is_empty()
    );
    let quote = orders.quote_cart(&second).await.unwrap();
    assert_eq!(quote.out_of_stock.len(), 1);
    let request_id = orders
        .request_checkout(SHOPPER, &second, checkout())
        .await
        .unwrap();
    assert!(matches!(
        orders.pending_payment(&second, &request_id).await,
        Err(OrderError::OutOfStock { .. })
    ));
}

#[tokio::test]
async fn a_paid_order_takes_its_share_and_a_cancelled_one_gives_it_back() {
    let (executor, _) = executor_with_pool().await;
    let orders = timada_orders::Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    catalog.set_stock(ALICE, &mug, None, 3).await.unwrap();
    let cart_id = cart_with(&orders, &mug, 2).await;
    let placed = pay(&orders, &cart_id, checkout()).await;

    let run = async || {
        // Told twice, it counts once.
        for _ in 0..2 {
            stock_usage_subscription::<evento::Sqlite>(evento::Evento::new(executor.clone()))
                .run_once(&executor)
                .await
                .unwrap();
        }
    };
    run().await;
    let stock = catalog.load_stock(&mug, None).await.unwrap().unwrap();
    assert_eq!(stock.on_hand, 1);

    orders
        .cancel_order(ALICE, &placed.order_id, "Damaged".into())
        .await
        .unwrap();
    run().await;
    let stock = catalog.load_stock(&mug, None).await.unwrap().unwrap();
    assert_eq!(stock.on_hand, 3);
}
