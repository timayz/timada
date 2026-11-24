mod common;

use common::*;
use timada_core::{Claimant, LEASE_SECS, RETRY_DELAYS_SECS, Sending};
use timada_proposals::{NOTICE_WINDOW_SECS, notice_day};

fn at(node: &str, now: u64) -> Claimant<'_> {
    Claimant { node, now }
}

/// One node takes a day's notice; the others leave it; once sent it is
/// settled, and the next day has its own.
#[tokio::test]
async fn a_day_is_told_once() {
    let proposals = command().await;
    let day = notice_day(NOW);
    let expires_at = NOW + NOTICE_WINDOW_SECS;
    assert_eq!(proposals.load_notice(&day).await.unwrap(), None);

    assert!(
        proposals
            .claim_notice(&day, expires_at, at("node-a", NOW))
            .await
            .unwrap()
    );
    assert!(
        !proposals
            .claim_notice(&day, expires_at, at("node-b", NOW))
            .await
            .unwrap()
    );
    assert!(
        proposals
            .claim_notice(&day, expires_at, at("node-a", NOW))
            .await
            .unwrap()
    );
    let notice = proposals.load_notice(&day).await.unwrap().unwrap();
    assert_eq!(notice.day, day);
    assert_eq!(notice.expires_at, expires_at);
    assert_eq!(
        notice.sending,
        Sending::Claimed {
            by: "node-a".to_owned(),
            until: NOW + LEASE_SECS,
            attempts: 0,
        }
    );

    proposals.record_notice_sent(&day).await.unwrap();
    let sent = proposals.load_notice(&day).await.unwrap().unwrap();
    assert_eq!(sent.sending, Sending::Sent);
    assert!(
        !proposals
            .claim_notice(&day, expires_at, at("node-b", NOW + LEASE_SECS + 1))
            .await
            .unwrap()
    );

    let next = notice_day(NOW + 86_400);
    assert_ne!(next, day);
    assert!(
        proposals
            .claim_notice(&next, expires_at + 86_400, at("node-b", NOW + 86_400))
            .await
            .unwrap()
    );
}

/// A send the server failed is deferred, and taken again once due.
#[tokio::test]
async fn a_failed_send_is_tried_again_later() {
    let proposals = command().await;
    let day = notice_day(NOW);
    let expires_at = NOW + NOTICE_WINDOW_SECS;
    proposals
        .claim_notice(&day, expires_at, at("node-a", NOW))
        .await
        .unwrap();
    proposals
        .record_notice_not_sent(&day, "node-a", "connection refused", NOW)
        .await
        .unwrap();
    let notice = proposals.load_notice(&day).await.unwrap().unwrap();
    let Sending::Deferred {
        retry_at, attempts, ..
    } = notice.sending
    else {
        panic!("deferred: {:?}", notice.sending);
    };
    assert_eq!(attempts, 1);
    assert_eq!(retry_at, NOW + RETRY_DELAYS_SECS[0]);
    assert!(
        !proposals
            .claim_notice(&day, expires_at, at("node-b", retry_at - 1))
            .await
            .unwrap()
    );
    assert!(
        proposals
            .claim_notice(&day, expires_at, at("node-b", retry_at))
            .await
            .unwrap()
    );
    proposals
        .record_notice_undeliverable(&day, "no such mailbox")
        .await
        .unwrap();
    let notice = proposals.load_notice(&day).await.unwrap().unwrap();
    assert!(notice.sending.is_settled());
}
