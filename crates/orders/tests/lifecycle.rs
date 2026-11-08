mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_orders::{
    CancelPath, CaptureClaim, HOLD_EXPIRED_REASON, HOLD_MARGIN_SECS, HOLD_REMINDER_BEFORE_SECS,
    HOLD_USUAL_SECS, OrderError, OrderStatus, Released, SupplierOrder,
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

/// The provider dropped the hold while the supplier may have the order: it
/// may be shipping, so nothing cancels the order but the merchant. The hold
/// is recorded gone, once, and the cancellation they ask for settles it.
#[tokio::test]
async fn a_dropped_hold_leaves_an_order_the_supplier_may_have_to_the_merchant() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    let payment_id = order.payment.payment_id.clone();

    let released = orders
        .record_release(DASHBOARD, &order_id, &payment_id, HOLD_EXPIRED_REASON)
        .await
        .unwrap();
    assert_eq!(released, Released::KeptOpen);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Authorized, "not cancelled");
    assert!(order.released_at.is_some());
    assert_eq!(order.payment_state(), "released");
    assert_eq!(order.cancel_reason, None);
    let version = order.aggregate_version().unwrap();

    // Told again (the webhook, the sweep): nothing more.
    let again = orders
        .record_release(DASHBOARD, &order_id, &payment_id, HOLD_EXPIRED_REASON)
        .await
        .unwrap();
    assert_eq!(again, Released::KeptOpen);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.aggregate_version().unwrap(), version);
    // Nothing is held any more: nothing to take, nothing to buy with it.
    let taken = orders.request_capture(DASHBOARD, &order_id).await;
    assert!(
        matches!(taken, Err(OrderError::OrderCancelled)),
        "{taken:?}"
    );
    let bought = orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await;
    assert!(
        matches!(bought, Err(OrderError::OrderCancelled)),
        "{bought:?}"
    );

    // The merchant settles it with the supplier and cancels it.
    let path = orders
        .request_cancellation(ALICE, &order_id, "Cancelled on AliExpress".into(), true)
        .await
        .unwrap();
    assert_eq!(
        path,
        CancelPath::Release {
            payment_id: payment_id.clone()
        }
    );
    let released = orders
        .record_release(DASHBOARD, &order_id, &payment_id, "")
        .await
        .unwrap();
    assert_eq!(released, Released::Cancelled);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(
        order.cancel_reason.as_deref(),
        Some("Cancelled on AliExpress")
    );
    assert!(order.pending_cancellation.is_none());

    // The supplier surely refused it: a dropped hold cancels the order again.
    let order_id = held(&orders, &catalog).await;
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    orders
        .record_supplier_refusal(DASHBOARD, &order_id, "aliexpress", "No delivery to FR")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    let released = orders
        .record_release(
            DASHBOARD,
            &order_id,
            &order.payment.payment_id,
            HOLD_EXPIRED_REASON,
        )
        .await
        .unwrap();
    assert_eq!(released, Released::Cancelled);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancel_reason.as_deref(), Some(HOLD_EXPIRED_REASON));
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
    assert!(!order.supplier_uncertain);

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

/// An order to the supplier that never said how it went may be there:
/// a sure refusal of a later one does not lower the fence, and only the
/// merchant, having looked, rules it out, once nothing is being asked.
#[tokio::test]
async fn an_order_that_may_be_at_the_supplier_keeps_its_fence_until_ruled_out() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;
    let fenced = async || {
        matches!(
            orders
                .request_cancellation(ALICE, &order_id, "Out of stock".into(), false)
                .await,
            Err(OrderError::SupplierOrdered)
        )
    };

    // Said uncertain with nothing begun: nothing to say.
    orders
        .record_supplier_uncertainty(DASHBOARD, &order_id, "aliexpress", "No answer")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert!(!order.supplier_uncertain);
    assert_eq!(order.supplier_order, SupplierOrder::None);

    // The first order gets no answer.
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    orders
        .record_supplier_uncertainty(DASHBOARD, &order_id, "aliexpress", "No answer")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert!(order.supplier_uncertain);
    assert!(!order.supplier_attempt_open);
    let version = order.aggregate_version().unwrap();
    orders
        .record_supplier_uncertainty(DASHBOARD, &order_id, "aliexpress", "No answer")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.aggregate_version().unwrap(), version, "said once");

    // Placed again, surely refused: the first may still be there.
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let asking = orders
        .rule_out_supplier_order(ALICE, &order_id, "aliexpress")
        .await;
    assert!(
        matches!(asking, Err(OrderError::SupplierAsking)),
        "{asking:?}"
    );
    orders
        .record_supplier_refusal(DASHBOARD, &order_id, "aliexpress", "Address refused")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.supplier_order, SupplierOrder::Started);
    assert!(fenced().await);

    // The merchant looked: nothing there. The fence goes.
    orders
        .rule_out_supplier_order(ALICE, &order_id, "aliexpress")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.supplier_order, SupplierOrder::Refused);
    assert!(!order.supplier_uncertain);
    assert!(!fenced().await);
}

/// Begun again while the order before never said how it went (its node
/// stopped, and the merchant placed it again before any sweep): that one may
/// be there, so a sure refusal of the new one keeps the fence.
#[tokio::test]
async fn an_order_begun_over_one_that_never_answered_keeps_the_fence() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let version = orders
        .load_order(&order_id)
        .await
        .unwrap()
        .unwrap()
        .aggregate_version()
        .unwrap();

    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(
        order.aggregate_version().unwrap(),
        version + 1,
        "written again, so a cancellation in between would have refused it"
    );
    assert!(order.supplier_uncertain);
    assert!(order.supplier_uncertain_at.is_some());

    orders
        .record_supplier_refusal(DASHBOARD, &order_id, "aliexpress", "Address refused")
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert_eq!(order.supplier_order, SupplierOrder::Started);
    assert!(!order.supplier_attempt_open);
    let unsaid = orders
        .request_cancellation(ALICE, &order_id, "Out of stock".into(), false)
        .await;
    assert!(
        matches!(unsaid, Err(OrderError::SupplierOrdered)),
        "{unsaid:?}"
    );
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

/// The shopper cancels a held order of goods a supplier buys, until the
/// supplier may have it; asked twice, nothing more.
#[tokio::test]
async fn the_shopper_cancels_until_the_supplier_may_have_the_order() {
    let (orders, catalog) = store().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let supplied = async || {
        let cart_id = cart_with(&orders, &mug, 1).await;
        let request_id = orders
            .request_checkout(SHOPPER, &cart_id, checkout())
            .await
            .unwrap();
        let mut pending = orders
            .pending_payment(&cart_id, &request_id)
            .await
            .unwrap()
            .unwrap();
        pending.supplier = Some("aliexpress".into());
        let payment = timada_orders::Payment {
            provider: timada_orders::PROVIDER_STRIPE.into(),
            payment_id: format!("pi_{request_id}"),
            amount_cents: pending.total_cents,
            currency: pending.currency.clone(),
            capture_before: None,
        };
        orders
            .record_payment_request(
                DASHBOARD,
                &cart_id,
                pending,
                timada_orders::OpenedPayment {
                    provider: timada_orders::PROVIDER_STRIPE.into(),
                    payment_id: payment.payment_id.clone(),
                    client_secret: "secret".into(),
                    publishable_key: "pk_test".into(),
                },
            )
            .await
            .unwrap();
        orders
            .place_order(DASHBOARD, &cart_id, payment)
            .await
            .unwrap()
            .order_id
    };

    let order_id = supplied().await;
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert!(order.cancellable_by_shopper());
    let path = orders
        .request_shopper_cancellation(&order_id)
        .await
        .unwrap();
    assert!(matches!(path, CancelPath::Release { .. }), "{path:?}");
    let again = orders
        .request_shopper_cancellation(&order_id)
        .await
        .unwrap();
    assert_eq!(again, path);
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    let asked = order.pending_cancellation.unwrap();
    assert_eq!(asked.requested_by, SHOPPER);
    assert_eq!(asked.reason, timada_orders::SHOPPER_CANCEL_REASON);

    // Being placed with the supplier: too late.
    let order_id = supplied().await;
    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let late = orders.request_shopper_cancellation(&order_id).await;
    assert!(
        matches!(late, Err(OrderError::OrderInPreparation)),
        "{late:?}"
    );
    // It surely refused: the shopper may cancel again.
    orders
        .record_supplier_refusal(DASHBOARD, &order_id, "aliexpress", "No delivery")
        .await
        .unwrap();
    orders
        .request_shopper_cancellation(&order_id)
        .await
        .unwrap();

    // Goods the shop holds are paid for at once: no shopper's cancel.
    let own = held(&orders, &catalog).await;
    let refused = orders.request_shopper_cancellation(&own).await;
    assert!(
        matches!(refused, Err(OrderError::OrderInPreparation)),
        "{refused:?}"
    );
}

/// The merchant is reminded, once, that the hold lapses soon on an order
/// AliExpress may have and whose money is still only held: two days before
/// it does, as the provider said, or a week less two days after the order
/// when it did not. Taking the money, or ruling the supplier out, ends it.
#[tokio::test]
async fn the_merchant_is_reminded_once_before_a_held_card_lapses() {
    let (orders, catalog) = store().await;
    let order_id = held(&orders, &catalog).await;
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert!(!order.hold_at_risk(), "nothing asked of the supplier");
    assert_eq!(order.hold_reminder_due_at(), None);
    let placed_at = order.placed_at;

    orders
        .begin_supplier_order(DASHBOARD, &order_id, "aliexpress", 0)
        .await
        .unwrap();
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert!(order.hold_at_risk());
    let due = placed_at + HOLD_USUAL_SECS - HOLD_REMINDER_BEFORE_SECS;
    assert_eq!(order.hold_reminder_due_at(), Some(due));
    // Not due yet: nothing is taken.
    assert!(
        !orders
            .claim_hold_reminder(DASHBOARD, &order_id, due - 1)
            .await
            .unwrap()
    );
    assert!(
        orders
            .claim_hold_reminder(DASHBOARD, &order_id, due)
            .await
            .unwrap()
    );
    let order = orders.load_order(&order_id).await.unwrap().unwrap();
    assert!(order.hold_reminded_at.is_some());
    assert!(order.hold_at_risk(), "still at risk, only reminded");
    assert_eq!(order.hold_reminder_due_at(), None, "reminded already");
    // The store API answers with this state: the shop's reminders stay out.
    let answered = serde_json::to_value(&order).unwrap();
    assert!(answered.get("hold_reminded_at").is_none());
    // Told again (another node, the next sweep): nothing more.
    assert!(
        !orders
            .claim_hold_reminder(DASHBOARD, &order_id, due + 1)
            .await
            .unwrap()
    );

    // A payment the provider dated: counted from its own end.
    let mug = product(&catalog, "Cup", 900, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    let (_, mut payment) = open_payment(&orders, &cart_id, checkout()).await;
    payment.capture_before = Some(1_000_000);
    let dated = orders
        .place_order(DASHBOARD, &cart_id, payment)
        .await
        .unwrap()
        .order_id;
    orders
        .begin_supplier_order(DASHBOARD, &dated, "aliexpress", 0)
        .await
        .unwrap();
    let order = orders.load_order(&dated).await.unwrap().unwrap();
    assert_eq!(
        order.hold_reminder_due_at(),
        Some(1_000_000 - HOLD_REMINDER_BEFORE_SECS)
    );
    // Taken meanwhile: no reminder, whatever the time.
    capture(&orders, &dated).await;
    let order = orders.load_order(&dated).await.unwrap().unwrap();
    assert!(!order.hold_at_risk());
    assert!(
        !orders
            .claim_hold_reminder(DASHBOARD, &dated, 2_000_000)
            .await
            .unwrap()
    );

    // Ruled out on the supplier's side: no longer at risk either.
    let ruled_out = held(&orders, &catalog).await;
    orders
        .begin_supplier_order(DASHBOARD, &ruled_out, "aliexpress", 0)
        .await
        .unwrap();
    orders
        .record_supplier_uncertainty(DASHBOARD, &ruled_out, "aliexpress", "no answer")
        .await
        .unwrap();
    assert!(
        orders
            .load_order(&ruled_out)
            .await
            .unwrap()
            .unwrap()
            .hold_at_risk()
    );
    orders
        .rule_out_supplier_order(ALICE, &ruled_out, "aliexpress")
        .await
        .unwrap();
    let order = orders.load_order(&ruled_out).await.unwrap().unwrap();
    assert!(!order.hold_at_risk());
    assert!(
        !orders
            .claim_hold_reminder(DASHBOARD, &ruled_out, u64::MAX)
            .await
            .unwrap()
    );
}
