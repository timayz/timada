mod common;

use common::*;
use timada_orders::{OrderStatus, OrdersError};

async fn paid(orders: &Orders, catalog: &Catalog) -> String {
    let mug = product(catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(orders, &mug, 1).await;
    pay(orders, &cart_id, checkout()).await.order_id
}

#[tokio::test]
async fn a_paid_order_is_fulfilled() {
    let (orders, catalog) = store().await;
    let order_id = paid(&orders, &catalog).await;

    orders
        .fulfil_order(ALICE, &order_id, Some(" 6A123 ".into()))
        .await
        .unwrap();
    orders.fulfil_order(ALICE, &order_id, None).await.unwrap();

    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Fulfilled);
    assert_eq!(order.tracking.as_deref(), Some("6A123"));
    assert!(order.fulfilled_at.is_some());

    let late = orders
        .cancel_order(ALICE, &order_id, "changed my mind".into())
        .await;
    assert!(matches!(late, Err(OrdersError::NotCancellable)), "{late:?}");
}

#[tokio::test]
async fn an_order_is_cancelled_with_a_reason_and_refunded() {
    let (orders, catalog) = store().await;
    let order_id = paid(&orders, &catalog).await;

    let bare = orders.cancel_order(ALICE, &order_id, "  ".into()).await;
    assert!(matches!(bare, Err(OrdersError::InvalidReason)), "{bare:?}");
    orders
        .cancel_order(ALICE, &order_id, "Out of stock".into())
        .await
        .unwrap();
    orders
        .cancel_order(ALICE, &order_id, "again".into())
        .await
        .unwrap();
    orders
        .record_refund(ALICE, &order_id, "re_1", 1250)
        .await
        .unwrap();
    orders
        .record_refund(ALICE, &order_id, "re_1", 1250)
        .await
        .unwrap();

    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancel_reason.as_deref(), Some("Out of stock"));
    assert_eq!(order.refunded_cents, 1250, "the same refund counts once");
    let fulfilled = orders.fulfil_order(ALICE, &order_id, None).await;
    assert!(
        matches!(fulfilled, Err(OrdersError::OrderCancelled)),
        "{fulfilled:?}"
    );
    let missing = orders.fulfil_order(ALICE, "no-such-order", None).await;
    assert!(
        matches!(missing, Err(OrdersError::OrderNotFound)),
        "{missing:?}"
    );
}
