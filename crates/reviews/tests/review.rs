mod common;

use common::*;
use timada_reviews::{ReviewError, ReviewStatus, SubmitReview, review_id};

#[tokio::test]
async fn a_buyer_reviews_a_product_of_their_order() {
    let reviews = command().await;
    paid_order(&reviews, "order-1").await;

    let review = submit(&reviews, "order-1", MUG, 4).await;
    assert_eq!(review.id, review_id("order-1", MUG));
    assert_eq!(review.status, ReviewStatus::Pending);
    assert_eq!(review.rating, 4);
    assert_eq!(review.title.as_deref(), Some("Lovely"));
    assert_eq!(review.body, "Holds my tea all day long.");
    assert_eq!(review.author_name, "Ada L.");
    assert_eq!(review.variant_label.as_deref(), Some("Beige"));
    assert_eq!(review.photos.len(), 2);

    // The other product of the order is a review of its own.
    let tea = submit(&reviews, "order-1", TEA, 5).await;
    assert_eq!(tea.variant_label, None);
}

#[tokio::test]
async fn only_what_was_bought_is_reviewed_once() {
    let reviews = command().await;
    paid_order(&reviews, "order-1").await;
    submit(&reviews, "order-1", MUG, 4).await;

    let refused = |cmd: SubmitReview| {
        let reviews = &reviews;
        async move { reviews.submit_review(SHOPPER, cmd).await.unwrap_err() }
    };
    assert!(matches!(
        refused(draft("order-1", MUG, 5)).await,
        ReviewError::AlreadyReviewed
    ));
    assert!(matches!(
        refused(draft("order-1", "product-other", 5)).await,
        ReviewError::NotPurchased
    ));
    assert!(matches!(
        refused(draft("order-unknown", MUG, 5)).await,
        ReviewError::OrderNotFound
    ));

    paid_order(&reviews, "order-2").await;
    cancel(&reviews, "order-2").await;
    assert!(matches!(
        refused(draft("order-2", MUG, 5)).await,
        ReviewError::OrderCancelled
    ));
}

#[tokio::test]
async fn what_a_shopper_writes_is_checked() {
    let reviews = command().await;
    paid_order(&reviews, "order-1").await;
    let with = |change: fn(&mut SubmitReview)| {
        let mut cmd = draft("order-1", MUG, 4);
        change(&mut cmd);
        cmd
    };
    for (cmd, code) in [
        (with(|c| c.rating = 0), "review_invalid_rating"),
        (with(|c| c.rating = 6), "review_invalid_rating"),
        (
            with(|c| c.title = Some("x".repeat(101))),
            "review_invalid_title",
        ),
        (
            with(|c| c.body = "  too short ".into()),
            "review_invalid_body",
        ),
        (with(|c| c.body = "x".repeat(2001)), "review_invalid_body"),
        (
            with(|c| c.photos = (0..4).map(|n| photo(&n.to_string())).collect()),
            "review_too_many_photos",
        ),
    ] {
        let err = reviews.submit_review(SHOPPER, cmd).await.unwrap_err();
        assert_eq!(err.code(), Some(code));
    }
    // A blank title is no title.
    let review = reviews
        .submit_review(SHOPPER, with(|c| c.title = Some("   ".into())))
        .await
        .unwrap();
    assert_eq!(review.title, None);
}

#[tokio::test]
async fn the_merchant_publishes_takes_down_and_trims_photos() {
    let reviews = command().await;
    paid_order(&reviews, "order-1").await;
    let id = submit(&reviews, "order-1", MUG, 4).await.id;

    reviews
        .remove_review_photo(ALICE, &id, "photo-1")
        .await
        .unwrap();
    assert!(matches!(
        reviews.remove_review_photo(ALICE, &id, "photo-1").await,
        Err(ReviewError::PhotoNotFound)
    ));
    reviews.publish_review(ALICE, &id).await.unwrap();
    reviews.publish_review(ALICE, &id).await.unwrap();
    let review = reviews.load_review(&id).await.unwrap().unwrap();
    assert!(review.is_published());
    assert!(review.published_at.is_some());
    assert_eq!(review.photos.len(), 1);

    reviews
        .reject_review(ALICE, &id, " Off topic ")
        .await
        .unwrap();
    reviews.reject_review(ALICE, &id, "again").await.unwrap();
    let review = reviews.load_review(&id).await.unwrap().unwrap();
    assert!(review.is_rejected());
    assert_eq!(review.rejection_reason.as_deref(), Some("Off topic"));
    assert_eq!(review.published_at, None);

    // Rejected is final.
    assert!(matches!(
        reviews.publish_review(ALICE, &id).await,
        Err(ReviewError::Rejected)
    ));
    assert!(matches!(
        reviews.remove_review_photo(ALICE, &id, "photo-2").await,
        Err(ReviewError::Rejected)
    ));
    assert!(matches!(
        reviews.reject_review(ALICE, "missing", "").await,
        Err(ReviewError::NotFound)
    ));
    assert!(matches!(
        reviews.reject_review(ALICE, &id, &"x".repeat(301)).await,
        Err(ReviewError::InvalidReason)
    ));
}
