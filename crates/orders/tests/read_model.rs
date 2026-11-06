mod common;

use common::*;
use timada_orders::{
    Command, OrderFilter, OrderStatus, count_orders, fulfilled_between, open_carts,
    orders_of_email, orders_of_tenant, payments_due, product_ever_ordered, read_model_subscription,
};

#[tokio::test]
async fn orders_and_carts_are_listed() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;

    let left = cart_with(&orders, &mug, 3).await;
    let _empty = orders.create_cart(SHOPPER).await.unwrap();
    let ordered = cart_with(&orders, &mug, 2).await;
    let placed = pay(&orders, &ordered, checkout()).await;
    let cancelled_cart = cart_with(&orders, &mug, 1).await;
    let cancelled = pay(&orders, &cancelled_cart, checkout()).await;
    cancel(&orders, &cancelled.order_id, "Test").await;
    let held_cart = cart_with(&orders, &mug, 1).await;
    let held = place(&orders, &held_cart, checkout()).await;

    // Twice: delivery may repeat without the rows drifting.
    for _ in 0..2 {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
    }
    let all = OrderFilter::default();

    let rows = orders_of_tenant(&pool, TENANT, &all, 0, 10).await.unwrap();
    assert_eq!(rows.len(), 3);
    let row = rows.iter().find(|row| row.id == placed.order_id).unwrap();
    assert_eq!(row.reference, placed.reference);
    assert_eq!(row.status(), OrderStatus::Paid);
    assert_eq!(row.payment_state, "paid");
    assert!(row.paid_at.is_some());
    assert!(row.payment_id.starts_with("pi_"));
    assert_eq!(row.total_label(), "25.00 €");
    assert_eq!(row.item_count, 2);
    assert_eq!(row.name, "Ada Lovelace");
    let held_row = rows.iter().find(|row| row.id == held.order_id).unwrap();
    assert_eq!(held_row.status(), OrderStatus::Authorized);
    assert_eq!(held_row.payment_state, "held");
    assert_eq!(held_row.paid_at, None);
    let cancelled_row = rows
        .iter()
        .find(|row| row.id == cancelled.order_id)
        .unwrap();
    assert_eq!(cancelled_row.payment_state, "paid", "taken, then refunded");

    let only_cancelled = OrderFilter::default().status(Some(OrderStatus::Cancelled));
    let rows = orders_of_tenant(&pool, TENANT, &only_cancelled, 0, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, cancelled.order_id);

    let by_reference = OrderFilter::default().search(Some(&placed.reference));
    assert_eq!(count_orders(&pool, TENANT, &by_reference).await.unwrap(), 1);
    let by_email = OrderFilter::default().search(Some("ADA@example"));
    assert_eq!(count_orders(&pool, TENANT, &by_email).await.unwrap(), 3);
    let of_email = orders_of_email(&pool, TENANT, &checkout().email.to_lowercase(), 10)
        .await
        .unwrap();
    assert_eq!(of_email.len(), 3);
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

    // Which products were ever ordered: a cancelled order counts, a cart
    // does not, and nothing crosses tenants.
    assert!(product_ever_ordered(&pool, TENANT, &mug).await.unwrap());
    assert!(!product_ever_ordered(&pool, "tenant-2", &mug).await.unwrap());
    let in_a_cart_only = product(&catalog, "Plate", 500, "EUR").await;
    cart_with(&orders, &in_a_cart_only, 1).await;
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    assert!(
        !product_ever_ordered(&pool, TENANT, &in_a_cart_only)
            .await
            .unwrap()
    );
    assert_eq!(carts[0].item_count, 3);
}

#[tokio::test]
async fn fulfilled_orders_are_found_by_when_they_left() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let paid = pay(&orders, &cart_with(&orders, &mug, 1).await, checkout()).await;
    let left = pay(&orders, &cart_with(&orders, &mug, 1).await, checkout()).await;
    orders
        .fulfil_order(ALICE, &left.order_id, None)
        .await
        .unwrap();
    read_model_subscription::<evento::Turso>(pool.clone())
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

/// A hold the provider dropped while the supplier may have the order leaves
/// nothing to settle with the provider: the sweep passes it over until the
/// merchant asks to cancel it.
#[tokio::test]
async fn an_order_kept_open_without_its_hold_waits_for_the_merchant() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    // Two holds whose time ran out at 100,000.
    let lapsed = async || {
        let cart_id = cart_with(&orders, &mug, 1).await;
        let (_, mut payment) = open_payment(&orders, &cart_id, checkout()).await;
        payment.capture_before = Some(100_000);
        orders
            .place_order(DASHBOARD, &cart_id, payment)
            .await
            .unwrap()
            .order_id
    };
    let unread = lapsed().await;
    let kept = lapsed().await;
    orders
        .begin_supplier_order(DASHBOARD, &kept, "aliexpress", 0)
        .await
        .unwrap();
    let payment_id = orders
        .load_order(&kept)
        .await
        .unwrap()
        .unwrap()
        .payment
        .payment_id;
    orders
        .record_release(DASHBOARD, &kept, &payment_id, "")
        .await
        .unwrap();
    let due = async || {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
        let mut ids: Vec<String> = payments_due(&pool, 200_000, 200_000, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect();
        ids.sort();
        ids
    };

    assert_eq!(due().await, std::slice::from_ref(&unread));

    orders
        .request_cancellation(ALICE, &kept, "Cancelled on AliExpress".into(), true)
        .await
        .unwrap();
    let mut both = vec![unread, kept];
    both.sort();
    assert_eq!(due().await, both);
}
