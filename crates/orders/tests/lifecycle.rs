mod common;

use common::*;
use timada_orders::{OrderStatus, OrdersError};

async fn placed(orders: &Orders, catalog: &Catalog) -> String {
    let mug = product(catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(orders, &mug, 1).await;
    orders
        .place_order(SHOPPER, &cart_id, checkout())
        .await
        .unwrap()
        .order_id
}

#[tokio::test]
async fn an_order_is_paid_then_fulfilled() {
    let (orders, catalog) = store().await;
    let order_id = placed(&orders, &catalog).await;

    let early = orders.fulfil_order(ALICE, &order_id, None).await;
    assert!(matches!(early, Err(OrdersError::NotPaid)), "{early:?}");

    orders.mark_order_paid(ALICE, &order_id).await.unwrap();
    orders.mark_order_paid(ALICE, &order_id).await.unwrap();
    orders
        .fulfil_order(ALICE, &order_id, Some(" 6A123 ".into()))
        .await
        .unwrap();

    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Fulfilled);
    assert_eq!(order.tracking.as_deref(), Some("6A123"));
    assert!(order.paid_at.is_some() && order.fulfilled_at.is_some());

    let late = orders
        .cancel_order(ALICE, &order_id, "changed my mind".into())
        .await;
    assert!(matches!(late, Err(OrdersError::NotCancellable)), "{late:?}");
    let repaid = orders.mark_order_paid(ALICE, &order_id).await;
    assert!(
        matches!(repaid, Err(OrdersError::NotAwaitingPayment)),
        "{repaid:?}"
    );
}

#[tokio::test]
async fn an_order_is_cancelled_with_a_reason() {
    let (orders, catalog) = store().await;
    let order_id = placed(&orders, &catalog).await;

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

    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancel_reason.as_deref(), Some("Out of stock"));
    let paid = orders.mark_order_paid(ALICE, &order_id).await;
    assert!(
        matches!(paid, Err(OrdersError::NotAwaitingPayment)),
        "{paid:?}"
    );
    let missing = orders.mark_order_paid(ALICE, "no-such-order").await;
    assert!(
        matches!(missing, Err(OrdersError::OrderNotFound)),
        "{missing:?}"
    );
}
