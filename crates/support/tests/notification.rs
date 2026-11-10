mod common;

use common::*;
use timada_core::{Claimant, LEASE_SECS, RETRY_DELAYS_SECS, Sending};
use timada_support::MESSAGE_NOTICE_WINDOW_SECS;

const NOW: u64 = 1_700_000_000;

fn at(node: &str, now: u64) -> Claimant<'_> {
    Claimant { node, now }
}

/// One node takes a message's notice; the others leave it; once sent it is
/// settled, and each message has its own.
#[tokio::test]
async fn a_message_is_told_once() {
    let support = command().await;
    let keys = MemoryKeys::default();
    let message = receive(&support, &keys, "Where is my order? It is late.").await;
    let other = receive(&support, &keys, "Do you ship to Belgium?").await;
    let expires_at = NOW + MESSAGE_NOTICE_WINDOW_SECS;
    assert_eq!(
        support
            .load_message_notification(&message.id)
            .await
            .unwrap(),
        None,
        "pending: no claim yet"
    );

    assert!(
        support
            .claim_notification(&message.id, expires_at, at("node-a", NOW))
            .await
            .unwrap()
    );
    // Another node: taken. The same node: it holds the claim.
    assert!(
        !support
            .claim_notification(&message.id, expires_at, at("node-b", NOW))
            .await
            .unwrap()
    );
    assert!(
        support
            .claim_notification(&message.id, expires_at, at("node-a", NOW))
            .await
            .unwrap()
    );
    let notice = support
        .load_message_notification(&message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notice.message_id, message.id);
    assert_eq!(notice.expires_at, expires_at);
    assert_eq!(
        notice.sending,
        Sending::Claimed {
            by: "node-a".to_owned(),
            until: NOW + LEASE_SECS,
            attempts: 0,
        }
    );

    support.record_notification_sent(&message.id).await.unwrap();
    let sent = support
        .load_message_notification(&message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent.sending, Sending::Sent);
    assert!(sent.sent_at.is_some());
    assert!(
        !support
            .claim_notification(&message.id, expires_at, at("node-b", NOW + LEASE_SECS))
            .await
            .unwrap()
    );

    // Each message has its own notice.
    assert!(
        support
            .claim_notification(&other.id, expires_at, at("node-b", NOW))
            .await
            .unwrap()
    );
}

/// A send the server fails waits for the schedule's next try, which any
/// node may take once due; the retries stop at the notice's window; a
/// refused address settles it at once.
#[tokio::test]
async fn a_failed_notice_is_deferred_then_given_up_within_its_window() {
    let support = command().await;
    let keys = MemoryKeys::default();
    let message = receive(&support, &keys, "Where is my order? It is late.").await;
    let expires_at = NOW + MESSAGE_NOTICE_WINDOW_SECS;
    assert!(
        support
            .claim_notification(&message.id, expires_at, at("node-a", NOW))
            .await
            .unwrap()
    );
    support
        .record_notification_not_sent(&message.id, "node-a", "connection refused", NOW)
        .await
        .unwrap();
    let deferred = support
        .load_message_notification(&message.id)
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
    let retry = NOW + RETRY_DELAYS_SECS[0];
    assert!(
        !support
            .claim_notification(&message.id, expires_at, at("node-b", retry - 1))
            .await
            .unwrap()
    );
    assert!(
        support
            .claim_notification(&message.id, expires_at, at("node-b", retry))
            .await
            .unwrap()
    );
    // A stale holder's word counts for nothing.
    support
        .record_notification_not_sent(&message.id, "node-a", "still refused", retry)
        .await
        .unwrap();
    assert!(matches!(
        support.load_message_notification(&message.id).await.unwrap().unwrap().sending,
        Sending::Claimed { ref by, attempts: 1, .. } if by == "node-b"
    ));
    // A retry that would fall past the window gives the notice up.
    let late = expires_at - RETRY_DELAYS_SECS[1];
    support
        .record_notification_not_sent(&message.id, "node-b", "no answer", late)
        .await
        .unwrap();
    assert_eq!(
        support
            .load_message_notification(&message.id)
            .await
            .unwrap()
            .unwrap()
            .sending,
        Sending::Failed {
            reason: "no answer".to_owned(),
            attempts: 2,
        }
    );

    let other = receive(&support, &keys, "Do you ship to Belgium?").await;
    assert!(
        support
            .claim_notification(&other.id, expires_at, at("node-a", NOW))
            .await
            .unwrap()
    );
    support
        .record_notification_undeliverable(&other.id, "550 5.1.1 no such user")
        .await
        .unwrap();
    let refused = support
        .load_message_notification(&other.id)
        .await
        .unwrap()
        .unwrap();
    assert!(refused.sending.is_undeliverable(), "{refused:?}");
}
