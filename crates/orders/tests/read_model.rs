mod common;

use common::*;
use timada_orders::{
    Command, OrderFilter, OrderStatus, count_orders, fulfilled_between, open_carts,
    orders_of_email, orders_of_tenant, read_model_subscription,
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
    let placed = pay(&orders, &ordered, checkout()).await;
    let cancelled_cart = cart_with(&orders, &mug, 1).await;
    let cancelled = pay(&orders, &cancelled_cart, checkout()).await;
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
    assert_eq!(row.status(), OrderStatus::Paid);
    assert!(row.payment_id.starts_with("pi_"));
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
    let of_email = orders_of_email(&pool, TENANT, &checkout().email.to_lowercase(), 10)
        .await
        .unwrap();
    assert_eq!(of_email.len(), 2);
    assert!(
        orders_of_email(&pool, TENANT, "bob@example.com", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        orders_of_email(&pool, "tenant-2", &of_email[0].email, 10)
            .await
            .unwrap()
            .is_empty()
    );

    let carts = open_carts(&pool, TENANT, 0, 10).await.unwrap();
    assert_eq!(
        carts.len(),
        1,
        "an empty or ordered cart is not left behind"
    );
    assert_eq!(carts[0].id, left);
    assert_eq!(carts[0].item_count, 3);
}

#[tokio::test]
async fn fulfilled_orders_are_found_by_when_they_left() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command(executor.clone());
    let catalog = timada_catalog::Command(executor.clone());
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let paid = pay(&orders, &cart_with(&orders, &mug, 1).await, checkout()).await;
    let left = pay(&orders, &cart_with(&orders, &mug, 1).await, checkout()).await;
    orders
        .fulfil_order(ALICE, &left.order_id, None)
        .await
        .unwrap();
    read_model_subscription::<evento::Sqlite>(pool.clone(), TENANT)
        .run_once(&executor)
        .await
        .unwrap();

    let fulfilled_at = orders
        .load_order(&left.order_id)
        .await
        .unwrap()
        .unwrap()
        .fulfilled_at
        .unwrap() as i64;
    let found = fulfilled_between(&pool, fulfilled_at, fulfilled_at + 1)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, left.order_id);
    assert_eq!(found[0].tenant_id, TENANT);
    assert_eq!(found[0].fulfilled_at, fulfilled_at);
    assert!(
        fulfilled_between(&pool, fulfilled_at + 1, fulfilled_at + 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(found.iter().all(|order| order.id != paid.order_id));
}
