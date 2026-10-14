mod common;

use common::*;
use timada_orders::{
    Command, OrderFilter, OrderStatus, count_orders, open_carts, orders_of_tenant,
    read_model_subscription,
};

const TENANT: &str = "tenant-1";

#[tokio::test]
async fn orders_and_carts_are_listed() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command(executor.clone());
    let catalog = timada_catalog::Command(executor.clone());
    let mug = product(&catalog, "Mug", 1250, "EUR").await;

    let left = cart_with(&orders, &mug, 3).await;
    let _empty = orders.create_cart(SHOPPER).await.unwrap();
    let ordered = cart_with(&orders, &mug, 2).await;
    let placed = orders
        .place_order(SHOPPER, &ordered, checkout())
        .await
        .unwrap();
    let cancelled_cart = cart_with(&orders, &mug, 1).await;
    let cancelled = orders
        .place_order(SHOPPER, &cancelled_cart, checkout())
        .await
        .unwrap();
    orders
        .cancel_order(ALICE, &cancelled.order_id, "Test".into())
        .await
        .unwrap();

    // Twice: delivery may repeat without the rows drifting.
    for _ in 0..2 {
        read_model_subscription::<evento::Sqlite>(pool.clone(), TENANT)
            .run_once(&executor)
            .await
            .unwrap();
    }
    let all = OrderFilter::default();

    let rows = orders_of_tenant(&pool, TENANT, &all, 0, 10).await.unwrap();
    assert_eq!(rows.len(), 2);
    let row = rows.iter().find(|row| row.id == placed.order_id).unwrap();
    assert_eq!(row.reference, placed.reference);
    assert_eq!(row.status(), OrderStatus::AwaitingPayment);
    assert_eq!(row.total_label(), "25.00 EUR");
    assert_eq!(row.item_count, 2);
    assert_eq!(row.name, "Ada Lovelace");

    let only_cancelled = OrderFilter::default().status(Some(OrderStatus::Cancelled));
    let rows = orders_of_tenant(&pool, TENANT, &only_cancelled, 0, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, cancelled.order_id);

    let by_reference = OrderFilter::default().search(Some(&placed.reference));
    assert_eq!(count_orders(&pool, TENANT, &by_reference).await.unwrap(), 1);
    let by_email = OrderFilter::default().search(Some("ADA@example"));
    assert_eq!(count_orders(&pool, TENANT, &by_email).await.unwrap(), 2);

    let carts = open_carts(&pool, TENANT, 0, 10).await.unwrap();
    assert_eq!(
        carts.len(),
        1,
        "an empty or ordered cart is not left behind"
    );
    assert_eq!(carts[0].id, left);
    assert_eq!(carts[0].item_count, 3);
}
