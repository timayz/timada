mod common;

use common::*;
use timada_core::{Claimant, LEASE_SECS, RETRY_DELAYS_SECS, Sending};
use timada_orders::{ORDER_MAIL_WINDOW_SECS, OrderError};

const NOW: u64 = 1_700_000_000;

fn at(node: &str, now: u64) -> Claimant<'_> {
    Claimant { node, now }
}

#[tokio::test]
async fn the_storefront_address_is_set_and_forgotten() {
    let (orders, _) = store().await;
    assert_eq!(orders.storefront_address().await.unwrap(), None);

    let err = orders
        .set_storefront_address(ALICE, Some("shop.example"))
        .await
        .unwrap_err();
    assert!(matches!(err, OrderError::InvalidStorefrontUrl));

    orders
        .set_storefront_address(ALICE, Some("https://shop.example/"))
        .await
        .unwrap();
    // The same address again writes nothing.
    orders
        .set_storefront_address(ALICE, Some("https://shop.example"))
        .await
        .unwrap();
    assert_eq!(
        orders.storefront_address().await.unwrap().as_deref(),
        Some("https://shop.example")
    );

    orders.set_storefront_address(ALICE, None).await.unwrap();
    assert_eq!(orders.storefront_address().await.unwrap(), None);
}

/// One node takes each mail of an order; the others leave it; the first
/// claim says what the stream is about.
#[tokio::test]
async fn each_mail_is_taken_once_per_order() {
    let (orders, _) = store().await;
    let expires_at = NOW + ORDER_MAIL_WINDOW_SECS;
    assert_eq!(
        orders
            .load_order_mail("order-1", "order_confirmation")
            .await
            .unwrap(),
        None,
        "pending: no claim yet"
    );
    assert!(
        orders
            .claim_order_mail(
                "order-1",
                "order_confirmation",
                expires_at,
                at("node-a", NOW)
            )
            .await
            .unwrap()
    );
    // Another node: taken. The same node: it holds the claim.
    assert!(
        !orders
            .claim_order_mail(
                "order-1",
                "order_confirmation",
                expires_at,
                at("node-b", NOW)
            )
            .await
            .unwrap()
    );
    assert!(
        orders
            .claim_order_mail(
                "order-1",
                "order_confirmation",
                expires_at,
                at("node-a", NOW)
            )
            .await
            .unwrap()
    );
    let claimed = orders
        .load_order_mail("order-1", "order_confirmation")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.order_id, "order-1");
    assert_eq!(claimed.mail, "order_confirmation");
    assert_eq!(claimed.expires_at, expires_at);
    assert_eq!(
        claimed.sending,
        Sending::Claimed {
            by: "node-a".to_owned(),
            until: NOW + LEASE_SECS,
            attempts: 0,
        }
    );
    assert_eq!(claimed.sent_at, None);

    orders
        .record_order_mail_sent("order-1", "order_confirmation")
        .await
        .unwrap();
    let sent = orders
        .load_order_mail("order-1", "order_confirmation")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent.sending, Sending::Sent);
    assert!(sent.sent_at.is_some());
    // Settled: nobody takes it again, and a late report changes nothing.
    assert!(
        !orders
            .claim_order_mail(
                "order-1",
                "order_confirmation",
                expires_at,
                at("node-b", NOW + LEASE_SECS)
            )
            .await
            .unwrap()
    );
    orders
        .record_order_mail_not_sent("order-1", "order_confirmation", "node-a", "late", NOW)
        .await
        .unwrap();
    assert_eq!(
        orders
            .load_order_mail("order-1", "order_confirmation")
            .await
            .unwrap()
            .unwrap()
            .sending,
        Sending::Sent
    );

    // Another mail of the same order is its own.
    assert!(
        orders
            .claim_order_mail("order-1", "order_cancelled", expires_at, at("node-b", NOW))
            .await
            .unwrap()
    );
    assert!(
        orders
            .load_order_mail("order-2", "order_confirmation")
            .await
            .unwrap()
            .is_none()
    );
}

/// A send the shop's server fails waits for the schedule's next try, which
/// any node may take once due; a lapsed lease is taken over; the retries
/// stop at the mail's window, and a refused address settles it at once.
#[tokio::test]
async fn a_failed_send_is_deferred_then_given_up_within_the_mails_window() {
    let (orders, _) = store().await;
    let expires_at = NOW + ORDER_MAIL_WINDOW_SECS;
    let mail = ("order-1", "order_paid");
    assert!(
        orders
            .claim_order_mail(mail.0, mail.1, expires_at, at("node-a", NOW))
            .await
            .unwrap()
    );
    orders
        .record_order_mail_not_sent(mail.0, mail.1, "node-a", "connection refused", NOW)
        .await
        .unwrap();
    let deferred = orders
        .load_order_mail(mail.0, mail.1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        deferred.sending,
        Sending::Deferred {
            retry_at: NOW + RETRY_DELAYS_SECS[0],
            attempts: 1,
            reason: "connection refused".to_owned(),
        }
    );
    // Not before it is due; then whichever node looks.
    assert!(
        !orders
            .claim_order_mail(
                mail.0,
                mail.1,
                expires_at,
                at("node-b", NOW + RETRY_DELAYS_SECS[0] - 1)
            )
            .await
            .unwrap()
    );
    let retry = NOW + RETRY_DELAYS_SECS[0];
    assert!(
        orders
            .claim_order_mail(mail.0, mail.1, expires_at, at("node-b", retry))
            .await
            .unwrap()
    );
    // A stale holder's word counts for nothing.
    orders
        .record_order_mail_not_sent(mail.0, mail.1, "node-a", "still refused", retry)
        .await
        .unwrap();
    assert!(matches!(
        orders.load_order_mail(mail.0, mail.1).await.unwrap().unwrap().sending,
        Sending::Claimed { ref by, attempts: 1, .. } if by == "node-b"
    ));
    // Node b dies: its lease lapses and node a takes over.
    assert!(
        orders
            .claim_order_mail(mail.0, mail.1, expires_at, at("node-a", retry + LEASE_SECS))
            .await
            .unwrap()
    );
    // A retry that would fall past the window gives the mail up.
    let late = expires_at - RETRY_DELAYS_SECS[1];
    orders
        .record_order_mail_not_sent(mail.0, mail.1, "node-a", "no answer", late)
        .await
        .unwrap();
    let failed = orders
        .load_order_mail(mail.0, mail.1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        failed.sending,
        Sending::Failed {
            reason: "no answer".to_owned(),
            attempts: 2,
        }
    );
    assert!(
        !orders
            .claim_order_mail(mail.0, mail.1, expires_at, at("node-a", late))
            .await
            .unwrap()
    );

    // A refused address is settled by whoever reports it.
    let other = ("order-2", "order_shipped");
    assert!(
        orders
            .claim_order_mail(other.0, other.1, expires_at, at("node-a", NOW))
            .await
            .unwrap()
    );
    orders
        .record_order_mail_undeliverable(other.0, other.1, "550 5.1.1 no such user")
        .await
        .unwrap();
    let refused = orders
        .load_order_mail(other.0, other.1)
        .await
        .unwrap()
        .unwrap();
    assert!(refused.sending.is_undeliverable(), "{refused:?}");
    assert!(
        !orders
            .claim_order_mail(other.0, other.1, expires_at, at("node-b", NOW + LEASE_SECS))
            .await
            .unwrap()
    );
}
