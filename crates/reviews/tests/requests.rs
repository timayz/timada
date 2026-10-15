mod common;

use common::*;
use timada_reviews::{
    DEFAULT_REQUEST_DELAY_DAYS, MAX_REQUEST_DELAY_DAYS, REVIEW_LINK_TTL_SECS, ReviewError,
    review_link_id,
};

#[tokio::test]
async fn a_shop_asks_a_week_after_fulfilment_until_told_otherwise() {
    let reviews = command().await;
    let policy = reviews.review_requests().await.unwrap();
    assert_eq!(policy.delay_days(), Some(DEFAULT_REQUEST_DELAY_DAYS));

    reviews.set_review_requests(ALICE, Some(3)).await.unwrap();
    assert_eq!(
        reviews.review_requests().await.unwrap().delay_days(),
        Some(3)
    );

    reviews.set_review_requests(ALICE, None).await.unwrap();
    assert_eq!(reviews.review_requests().await.unwrap().delay_days(), None);

    // Turned on again, at once.
    reviews.set_review_requests(ALICE, Some(0)).await.unwrap();
    assert_eq!(
        reviews.review_requests().await.unwrap().delay_days(),
        Some(0)
    );

    assert!(matches!(
        reviews
            .set_review_requests(ALICE, Some(MAX_REQUEST_DELAY_DAYS + 1))
            .await
            .unwrap_err(),
        ReviewError::RequestDelayTooLong
    ));
}

#[tokio::test]
async fn the_same_setting_again_writes_nothing() {
    let reviews = command().await;
    // The default, said out loud: still no stream.
    reviews
        .set_review_requests(ALICE, Some(DEFAULT_REQUEST_DELAY_DAYS))
        .await
        .unwrap();
    assert!(reviews.load_review_requests().await.unwrap().is_none());
}

#[tokio::test]
async fn an_order_is_asked_once() {
    let reviews = command().await;
    assert!(
        reviews
            .claim_review_request("order-1", "ada@example.com", SHOPPER)
            .await
            .unwrap()
    );
    assert!(
        !reviews
            .claim_review_request("order-1", "ada@example.com", SHOPPER)
            .await
            .unwrap()
    );
    let request = reviews
        .load_review_request("order-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.email, "ada@example.com");
    assert!(
        reviews
            .load_review_request("order-2")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_link_opens_its_order_until_it_expires() {
    let reviews = command().await;
    let now = 1_000_000;
    let token = reviews
        .issue_review_link("order-1", SHOPPER, now)
        .await
        .unwrap();
    assert_eq!(token.len(), 43);
    // The store keeps the hash, never the token.
    assert!(reviews.load_review_link(&token).await.unwrap().is_none());
    assert!(
        reviews
            .load_review_link(&review_link_id(&token))
            .await
            .unwrap()
            .is_some()
    );

    let open = |at: u64| {
        let (reviews, token) = (&reviews, token.clone());
        async move { reviews.resolve_review_link(&token, at).await.unwrap() }
    };
    assert_eq!(open(now).await.as_deref(), Some("order-1"));
    assert_eq!(
        open(now + REVIEW_LINK_TTL_SECS - 1).await.as_deref(),
        Some("order-1")
    );
    assert_eq!(open(now + REVIEW_LINK_TTL_SECS).await, None);

    assert_eq!(
        reviews.resolve_review_link("bogus", now).await.unwrap(),
        None
    );
    assert_eq!(reviews.resolve_review_link("", now).await.unwrap(), None);
}
