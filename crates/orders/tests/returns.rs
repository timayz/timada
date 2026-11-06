//! Money that buys nothing goes back in two writes around the provider's:
//! a cart records that a payment is on its way back, and why, before the
//! provider is asked, then that it went; money taken for a cancelled order
//! goes back through the order the same way. What waits between the two is
//! what the sweep and the payments page read.

mod common;

use common::*;
use timada_orders::{
    Command, OrderError, PaymentAnswer, ReturnedPayment, money_held, payments_due,
    read_model_subscription, returns_due,
};

#[tokio::test]
async fn a_payment_goes_back_once_for_the_reason_asked_first() {
    let (executor, db) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let (request_id, payment) = open_payment(&orders, &cart_id, checkout()).await;
    let read = async || {
        read_model_subscription::<evento::Turso>(db.clone())
            .run_once(&executor)
            .await
            .unwrap();
    };

    // A payment the cart never opened is nothing to give back, and one
    // nobody asked to give back is not recorded gone.
    assert!(matches!(
        orders
            .request_payment_return(DASHBOARD, &cart_id, "pi_other", "out_of_stock")
            .await,
        Err(OrderError::PaymentNotFound)
    ));
    assert!(matches!(
        orders
            .return_payment(DASHBOARD, &cart_id, &payment.payment_id, None)
            .await,
        Err(OrderError::PaymentNotFound)
    ));

    let pending = orders
        .request_payment_return(DASHBOARD, &cart_id, &payment.payment_id, "out_of_stock")
        .await
        .unwrap()
        .expect("on its way back");
    assert_eq!(pending.code, "out_of_stock");
    // Asked again, for another reason: the first one stands.
    assert_eq!(
        orders
            .request_payment_return(
                DASHBOARD,
                &cart_id,
                &payment.payment_id,
                "duplicate_payment"
            )
            .await
            .unwrap(),
        Some(pending.clone())
    );
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(cart.pending_return(&payment.payment_id), Some(&pending));
    assert_eq!(cart.return_requested_at(), Some(pending.requested_at));
    assert!(!cart.is_paying(), "nothing left to confirm");
    assert!(
        cart.lapsed_payments(u64::MAX, 0).is_empty(),
        "no card form to close"
    );
    // Meanwhile it makes no order.
    assert!(matches!(
        orders
            .place_order(DASHBOARD, &cart_id, payment.clone())
            .await,
        Err(OrderError::PaymentNotFound)
    ));
    // The sweep finds it, and the shop has money held.
    read().await;
    let due = returns_due(&db, i64::MAX, 10).await.unwrap();
    assert_eq!(
        due.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        [cart_id.as_str()]
    );
    assert!(
        returns_due(&db, 0, 10).await.unwrap().is_empty(),
        "asked just now"
    );
    assert!(money_held(&db, TENANT).await.unwrap());

    orders
        .return_payment(DASHBOARD, &cart_id, &payment.payment_id, None)
        .await
        .unwrap();
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.request(&request_id).unwrap().answer,
        Some(PaymentAnswer::Returned(Box::new(ReturnedPayment {
            payment_id: payment.payment_id.clone(),
            code: "out_of_stock".into(),
        })))
    );
    assert_eq!(cart.pending_return(&payment.payment_id), None);
    assert_eq!(cart.status(), "open");
    // Told twice, nothing more; asked again, it went already.
    orders
        .return_payment(DASHBOARD, &cart_id, &payment.payment_id, None)
        .await
        .unwrap();
    assert_eq!(
        orders
            .request_payment_return(DASHBOARD, &cart_id, &payment.payment_id, "out_of_stock")
            .await
            .unwrap(),
        None
    );
    read().await;
    assert!(returns_due(&db, i64::MAX, 10).await.unwrap().is_empty());
    assert!(!money_held(&db, TENANT).await.unwrap());
}

#[tokio::test]
async fn a_second_payment_of_an_order_goes_back_and_the_first_never_does() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    // Two tabs, two payments opened before either was paid.
    let (_, first) = open_payment(&orders, &cart_id, checkout()).await;
    let (second_request, second) = open_payment(&orders, &cart_id, checkout()).await;
    orders
        .place_order(DASHBOARD, &cart_id, first.clone())
        .await
        .unwrap();

    // The order's own payment goes back with the order, never so.
    assert!(matches!(
        orders
            .request_payment_return(DASHBOARD, &cart_id, &first.payment_id, "duplicate_payment")
            .await,
        Err(OrderError::PaymentNotFound)
    ));
    // The second one goes back, refunded: it had been taken.
    orders
        .request_payment_return(DASHBOARD, &cart_id, &second.payment_id, "duplicate_payment")
        .await
        .unwrap()
        .unwrap();
    orders
        .return_payment(DASHBOARD, &cart_id, &second.payment_id, Some("re_1".into()))
        .await
        .unwrap();
    let cart = orders.load_cart(&cart_id).await.unwrap().unwrap();
    assert_eq!(
        cart.returned_payment(&second.payment_id)
            .map(|r| r.code.as_str()),
        Some("duplicate_payment")
    );
    assert!(matches!(
        cart.request(&second_request).unwrap().answer,
        Some(PaymentAnswer::Returned(_))
    ));
    assert_eq!(cart.status(), "ordered");
}

#[tokio::test]
async fn money_taken_for_a_cancelled_order_goes_back_through_it() {
    let (executor, db) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let placed = place(&orders, &cart_id, checkout()).await;
    let paid_with = orders
        .load_order(&placed.order_id)
        .await
        .unwrap()
        .unwrap()
        .payment
        .payment_id;
    // Only for a cancelled order.
    assert!(matches!(
        orders
            .request_cancelled_refund(DASHBOARD, &placed.order_id, &paid_with, 1250)
            .await,
        Err(OrderError::InvalidRefund)
    ));
    cancel(&orders, &placed.order_id, "Out of stock").await;

    // The provider took the money anyway (by hand, in its own dashboard).
    assert!(matches!(
        orders
            .request_cancelled_refund(DASHBOARD, &placed.order_id, "pi_other", 1250)
            .await,
        Err(OrderError::PaymentNotFound)
    ));
    let pending = orders
        .request_cancelled_refund(DASHBOARD, &placed.order_id, &paid_with, 1250)
        .await
        .unwrap()
        .expect("asked for");
    assert_eq!(pending.amount_cents, 1250);
    assert_eq!(
        orders
            .request_cancelled_refund(DASHBOARD, &placed.order_id, &paid_with, 1250)
            .await
            .unwrap(),
        Some(pending.clone()),
        "the same key while it stands"
    );
    // The sweep finds the refund standing, and the shop has money held.
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(&executor)
        .await
        .unwrap();
    let due = payments_due(&db, 0, i64::MAX, 10).await.unwrap();
    assert!(due.iter().any(|row| row.id == placed.order_id), "{due:?}");
    assert!(money_held(&db, TENANT).await.unwrap());

    orders
        .record_refund(DASHBOARD, &placed.order_id, "re_1", pending.amount_cents)
        .await
        .unwrap();
    let order = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(order.refunded_cents, 1250);
    assert_eq!(order.pending_refund, None);
    assert_eq!(order.status, timada_orders::OrderStatus::Cancelled);
    assert_eq!(
        orders
            .request_cancelled_refund(DASHBOARD, &placed.order_id, &paid_with, 1250)
            .await
            .unwrap(),
        None,
        "it went back"
    );
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(&executor)
        .await
        .unwrap();
    assert!(!money_held(&db, TENANT).await.unwrap());
}
