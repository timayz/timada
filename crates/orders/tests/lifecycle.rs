mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_orders::{
    CancelPath, CaptureClaim, HOLD_EXPIRED_REASON, HOLD_MARGIN_SECS, OrderError, OrderStatus,
    SupplierOrder,
};

async fn held(orders: &Orders, catalog: &Catalog) -> String {
    let mug = product(catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(orders, &mug, 1).await;
    place(orders, &cart_id, checkout()).await.order_id
}

async fn paid(orders: &Orders, catalog: &Catalog) -> String {
    let order_id = held(orders, catalog).await;
    capture(orders, &order_id).await;
    order_id
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
    assert_eq!(order.fulfilled_by.as_deref(), Some(ALICE));
    // The store API answers with this state: a member's id stays out of it.
    let answered = serde_json::to_value(&order).unwrap();
    assert!(answered.get("fulfilled_at").is_some());
    assert!(answered.get("fulfilled_by").is_none());

    let late = orders
        .request_cancellation(ALICE, &order_id, "changed my mind".into(), false)
        .await;
    assert!(matches!(late, Err(OrderError::NotCancellable)), "{late:?}");
}

/// The order exists once the money is held: nothing can be refunded and
/// nothing leaves before it is taken, and taking it goes in two steps, so
/// only the one who asked speaks to the provider.
#[tokio::test]
async fn a_held_payment_is_taken_once() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;

    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Authorized);
    assert!(order.may_capture());
    assert!(!order.awaits_supplier(), "the shop holds the goods");
    let refund = orders.request_refund(ALICE, &order_id, 100).await;
    assert!(
        matches!(refund, Err(OrderError::PaymentNotCaptured)),
        "{refund:?}"
    );
    let early = orders.fulfil_order(ALICE, &order_id, None).await;
    assert!(
        matches!(early, Err(OrderError::PaymentNotCaptured)),
        "{early:?}"
    );

    let claim = orders.request_capture(DASHBOARD, &order_id).await.unwrap();
    let CaptureClaim::Claimed(pending) = claim else {
        panic!("the first to ask claims it: {claim:?}");
    };
    assert_eq!(pending.payment_id, order.payment.payment_id);
    assert_eq!(pending.amount_cents, 1250);
    assert_eq!(pending.requested_by, DASHBOARD);
    // Another node, or the merchant, asking meanwhile: told, not asked again.
    let again = orders.request_capture(ALICE, &order_id).await.unwrap();
    assert!(matches!(again, CaptureClaim::Pending(_)), "{again:?}");
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.payment_state(), "capturing");
    assert!(!order.may_capture());

    // The provider refused: recorded, and it may be asked again.
    orders
        .record_capture_failure(DASHBOARD, &order_id, "Your card was declined.")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.payment_state(), "capture_failed");
    assert_eq!(
        order.capture_failure.as_deref(),
        Some("Your card was declined.")
    );
    assert!(order.pending_capture.is_none());
    // A failure with nothing asked is nothing.
    orders
        .record_capture_failure(DASHBOARD, &order_id, "late")
        .await
        .unwrap();
    let claim = orders.request_capture(ALICE, &order_id).await.unwrap();
    assert!(matches!(claim, CaptureClaim::Claimed(_)), "{claim:?}");

    let wrong = orders
        .record_payment(DASHBOARD, &order_id, "pi_other", 1250)
        .await;
    assert!(
        matches!(wrong, Err(OrderError::PaymentNotFound)),
        "{wrong:?}"
    );
    orders
        .record_payment(DASHBOARD, &order_id, &order.payment.payment_id, 1250)
        .await
        .unwrap();
    // Told twice (the webhook after the capture): the same order.
    orders
        .record_payment(DASHBOARD, &order_id, &order.payment.payment_id, 1250)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(order.paid_cents, 1250);
    assert!(order.paid_at.is_some());
    assert_eq!(order.capture_failure, None);
    assert_eq!(order.outstanding_cents(), 1250);
    let done = orders.request_capture(ALICE, &order_id).await.unwrap();
    assert_eq!(done, CaptureClaim::Paid);
}

/// A held order is cancelled by giving its hold back: the reason and the
/// person of the request are what `Cancelled` says, written with the
/// release, and nothing can take the money any more.
#[tokio::test]
async fn a_held_order_is_cancelled_by_releasing_its_hold() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;

    let bare = orders
        .request_cancellation(ALICE, &order_id, "  ".into(), false)
        .await;
    assert!(matches!(bare, Err(OrderError::InvalidReason)), "{bare:?}");
    let path = orders
        .request_cancellation(ALICE, &order_id, "Out of stock".into(), false)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(
        path,
        CancelPath::Release {
            payment_id: order.payment.payment_id.clone()
        }
    );
    // Asked again before it is settled: the same path, nothing more written.
    let again = orders
        .request_cancellation(SHOPPER, &order_id, "again".into(), false)
        .await
        .unwrap();
    assert_eq!(again, path);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Authorized, "not cancelled yet");
    assert_eq!(
        order
            .pending_cancellation
            .as_ref()
            .map(|asked| asked.requested_by.as_str()),
        Some(ALICE)
    );
    let taken = orders.request_capture(DASHBOARD, &order_id).await;
    assert!(
        matches!(taken, Err(OrderError::CancellationPending)),
        "{taken:?}"
    );
    let shipped = orders.fulfil_order(ALICE, &order_id, None).await;
    assert!(shipped.is_err(), "{shipped:?}");

    orders
        .record_release(DASHBOARD, &order_id, &order.payment.payment_id, "")
        .await
        .unwrap();
    orders
        .record_release(DASHBOARD, &order_id, &order.payment.payment_id, "")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancel_reason.as_deref(), Some("Out of stock"));
    assert!(order.released_at.is_some());
    assert_eq!(order.payment_state(), "released");
    assert!(order.pending_cancellation.is_none());
    assert_eq!(order.refunded_cents, 0, "nothing was taken");

    let late = orders.request_capture(DASHBOARD, &order_id).await;
    assert!(matches!(late, Err(OrderError::OrderCancelled)), "{late:?}");
    let paid = orders
        .record_payment(DASHBOARD, &order_id, &order.payment.payment_id, 1250)
        .await;
    assert!(matches!(paid, Err(OrderError::OrderCancelled)), "{paid:?}");
    let done = orders
        .request_cancellation(ALICE, &order_id, "again".into(), false)
        .await
        .unwrap();
    assert_eq!(done, CancelPath::Done);
}

/// The provider dropped the hold on its own: the order is cancelled with
/// the reason given, by whoever recorded it.
#[tokio::test]
async fn a_lapsed_hold_cancels_the_order() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;
    let order = orders.load_order(&order_id).await.unwrap().unwrap();

    orders
        .record_release(
            DASHBOARD,
            &order_id,
            &order.payment.payment_id,
            HOLD_EXPIRED_REASON,
        )
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancel_reason.as_deref(), Some(HOLD_EXPIRED_REASON));

    // A taken payment is not released: there is nothing held.
    let order_id = paid(&orders, &catalog).await;
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    let released = orders
        .record_release(DASHBOARD, &order_id, &order.payment.payment_id, "")
        .await;
    assert!(
        matches!(released, Err(OrderError::NotCancellable)),
        "{released:?}"
    );
}

/// The provider is asked one thing at a time: a cancellation waits for a
/// capture in flight, then takes the refund path.
#[tokio::test]
async fn a_capture_in_flight_holds_off_a_cancellation() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;
    orders.request_capture(DASHBOARD, &order_id).await.unwrap();

    let early = orders
        .request_cancellation(ALICE, &order_id, "Out of stock".into(), false)
        .await;
    assert!(
        matches!(early, Err(OrderError::CapturePending)),
        "{early:?}"
    );

    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    orders
        .record_payment(DASHBOARD, &order_id, &order.payment.payment_id, 1250)
        .await
        .unwrap();
    let path = orders
        .request_cancellation(ALICE, &order_id, "Out of stock".into(), false)
        .await
        .unwrap();
    assert_eq!(path, CancelPath::Refund);
}

/// A taken payment is refunded before the order is cancelled; the key the
/// provider is asked with is fixed on the order first, so a retry makes the
/// same refund.
#[tokio::test]
async fn a_paid_order_is_refunded_then_cancelled() {
    let (orders, catalog) = store().await;
    let order_id = paid(&orders, &catalog).await;

    let path = orders
        .request_cancellation(ALICE, &order_id, "Out of stock".into(), false)
        .await
        .unwrap();
    assert_eq!(path, CancelPath::Refund);
    let shipped = orders.fulfil_order(ALICE, &order_id, None).await;
    assert!(
        matches!(shipped, Err(OrderError::CancellationPending)),
        "{shipped:?}"
    );

    let none = orders.request_refund(ALICE, &order_id, 0).await;
    assert!(matches!(none, Err(OrderError::InvalidRefund)), "{none:?}");
    let too_much = orders.request_refund(ALICE, &order_id, 1251).await;
    assert!(
        matches!(too_much, Err(OrderError::InvalidRefund)),
        "{too_much:?}"
    );
    // Not cancelled while money is outstanding.
    let unpaid_back = orders.complete_cancellation(&order_id).await;
    assert!(
        matches!(unpaid_back, Err(OrderError::InvalidRefund)),
        "{unpaid_back:?}"
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
    let early = orders.complete_cancellation(&order_id).await;
    assert!(matches!(early, Err(OrderError::RefundPending)), "{early:?}");

    // The provider's word settles it; the order is then cancelled.
    orders
        .record_refund(ALICE, &order_id, "re_1", 1250)
        .await
        .unwrap();
    orders
        .record_refund(ALICE, &order_id, "re_1", 1250)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert!(order.pending_refund.is_none());
    assert_eq!(order.outstanding_cents(), 0);
    assert_eq!(order.refunded_cents, 1250, "the same refund counts once");
    orders.complete_cancellation(&order_id).await.unwrap();
    orders.complete_cancellation(&order_id).await.unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancel_reason.as_deref(), Some("Out of stock"));
    assert_eq!(order.released_at, None);
    // Nothing outstanding: nothing more to ask for.
    let spent = orders.request_refund(ALICE, &order_id, 1).await;
    assert!(matches!(spent, Err(OrderError::InvalidRefund)), "{spent:?}");
    let fulfilled = orders.fulfil_order(ALICE, &order_id, None).await;
    assert!(
        matches!(fulfilled, Err(OrderError::OrderCancelled)),
        "{fulfilled:?}"
    );
    let missing = orders.fulfil_order(ALICE, "no-such-order", None).await;
    assert!(
        matches!(missing, Err(OrderError::OrderNotFound)),
        "{missing:?}"
    );

    // An order that left is not refunded this way.
    let left = paid(&orders, &catalog).await;
    orders.fulfil_order(ALICE, &left, None).await.unwrap();
    let late = orders.request_refund(ALICE, &left, 100).await;
    assert!(matches!(late, Err(OrderError::NotCancellable)), "{late:?}");
}

/// Ordering the goods from the supplier is written on the order first, on
/// every try: an order to the supplier and a cancellation never cross, and
/// once the supplier may have the order, cancelling it means saying so.
#[tokio::test]
async fn an_order_to_the_supplier_and_a_cancellation_never_cross() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;

    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.supplier_order, SupplierOrder::Started);
    // Placed again (after a failure that may have placed it): written again,
    // so a cancellation in between would have refused it.
    let version = order.aggregate_version().unwrap();
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.aggregate_version().unwrap(), version + 1);

    // The supplier may have it: cancelling without saying so is refused.
    let unsaid = orders
        .request_cancellation(ALICE, &order_id, "Out of stock".into(), false)
        .await;
    assert!(
        matches!(unsaid, Err(OrderError::SupplierOrdered)),
        "{unsaid:?}"
    );
    // It surely refused: cancelled freely again.
    orders
        .record_supplier_refusal(DASHBOARD, &order_id, "aliexpress", "No delivery to FR")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.supplier_order, SupplierOrder::Refused);
    let path = orders
        .request_cancellation(ALICE, &order_id, "Out of stock".into(), false)
        .await
        .unwrap();
    assert!(matches!(path, CancelPath::Release { .. }), "{path:?}");

    // A cancellation standing refuses the next order to the supplier, and
    // so does the cancelled order.
    let late = orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await;
    assert!(
        matches!(late, Err(OrderError::CancellationPending)),
        "{late:?}"
    );
    orders
        .record_release(DASHBOARD, &order_id, &order.payment.payment_id, "")
        .await
        .unwrap();
    let after = orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await;
    assert!(
        matches!(after, Err(OrderError::OrderCancelled)),
        "{after:?}"
    );

    // Said: the merchant cancels it at the supplier too.
    let order_id = held(&orders, &catalog).await;
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let path = orders
        .request_cancellation(ALICE, &order_id, "Out of stock".into(), true)
        .await
        .unwrap();
    assert!(matches!(path, CancelPath::Release { .. }), "{path:?}");
}

/// The goods are not bought once the hold on the shopper's card is about to
/// lapse: the money could not be taken after. Taken already, no hold limits it.
#[tokio::test]
async fn a_hold_about_to_lapse_is_not_ordered_from_the_supplier() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let (_, mut payment) = open_payment(&orders, &cart_id, checkout()).await;
    let lapses_at = 1_000_000;
    payment.capture_before = Some(lapses_at);
    let order_id = orders
        .place_order(DASHBOARD, &cart_id, payment)
        .await
        .unwrap()
        .order_id;

    let late = orders
        .begin_supplier_order(
            DASHBOARD,
            &order_id,
            "aliexpress",
            lapses_at - HOLD_MARGIN_SECS,
        )
        .await;
    assert!(matches!(late, Err(OrderError::HoldExpiring)), "{late:?}");
    orders
        .begin_supplier_order(
            DASHBOARD,
            &order_id,
            "aliexpress",
            lapses_at - HOLD_MARGIN_SECS - 1,
        )
        .await
        .unwrap();

    capture(&orders, &order_id).await;
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", lapses_at * 2)
        .await
        .unwrap();
    orders.fulfil_order(ALICE, &order_id, None).await.unwrap();
    let left = orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await;
    assert!(matches!(left, Err(OrderError::OrderFulfilled)), "{left:?}");
}
