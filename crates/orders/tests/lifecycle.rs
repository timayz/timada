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

/// A refund is asked for on the order before the provider is: the key the
/// provider is asked with is fixed there, so a retry makes the same refund,
/// and the order is not cancelled until the money is known to be on its way.
#[tokio::test]
async fn a_refund_is_requested_before_the_provider_and_settled_after() {
    let (orders, catalog) = store().await;
    let order_id = paid(&orders, &catalog).await;

    let none = orders.request_refund(ALICE, &order_id, 0).await;
    assert!(matches!(none, Err(OrdersError::InvalidRefund)), "{none:?}");
    let too_much = orders.request_refund(ALICE, &order_id, 1251).await;
    assert!(
        matches!(too_much, Err(OrdersError::InvalidRefund)),
        "{too_much:?}"
    );
    let pending = orders.request_refund(ALICE, &order_id, 1250).await.unwrap();
    assert_eq!(pending.amount_cents, 1250);
    assert!(pending.idempotency_key.contains(&order_id), "{pending:?}");
    // Asked again (a retry, another node): the same key, not a second refund.
    let again = orders.request_refund(ALICE, &order_id, 500).await.unwrap();
    assert_eq!(again.idempotency_key, pending.idempotency_key);
    assert_eq!(again.amount_cents, 1250);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(
        order.pending_refund.as_ref().unwrap().payment_id,
        order.payment.payment_id
    );

    // Not cancelled while the money is not known to be on its way.
    let early = orders
        .cancel_order(ALICE, &order_id, "Out of stock".into())
        .await;
    assert!(
        matches!(early, Err(OrdersError::RefundPending)),
        "{early:?}"
    );

    // The provider's word settles it; the order can then be cancelled.
    orders
        .record_refund(ALICE, &order_id, "re_1", 1250)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert!(order.pending_refund.is_none());
    assert_eq!(order.outstanding_cents(), 0);
    orders
        .cancel_order(ALICE, &order_id, "Out of stock".into())
        .await
        .unwrap();
    // Nothing outstanding: nothing more to ask for.
    let spent = orders.request_refund(ALICE, &order_id, 1).await;
    assert!(
        matches!(spent, Err(OrdersError::InvalidRefund)),
        "{spent:?}"
    );

    // An order that left is not refunded this way.
    let left = paid(&orders, &catalog).await;
    orders.fulfil_order(ALICE, &left, None).await.unwrap();
    let late = orders.request_refund(ALICE, &left, 100).await;
    assert!(matches!(late, Err(OrdersError::NotCancellable)), "{late:?}");
}
