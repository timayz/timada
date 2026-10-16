mod common;

use common::*;
use evento::ProjectionAggregate as _;
use timada_reviews::{ReviewError, ReviewStatus, ReviseReview, SubmitReview, review_id};

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

fn revision(order_id: &str, product_id: &str, keep: &[&str]) -> ReviseReview {
    ReviseReview {
        order_id: order_id.to_owned(),
        product_id: product_id.to_owned(),
        rating: 2,
        title: Some("  ".to_owned()),
        body: "  Chipped after a week.  ".to_owned(),
        keep: keep.iter().map(|id| (*id).to_owned()).collect(),
        photos: vec![photo("photo-3")],
    }
}

#[tokio::test]
async fn a_waiting_review_is_revised() {
    let reviews = command().await;
    paid_order(&reviews, "order-1").await;
    submit(&reviews, "order-1", MUG, 4).await;

    let revised = reviews
        .revise_review(SHOPPER, revision("order-1", MUG, &["photo-2"]))
        .await
        .unwrap();
    assert_eq!(revised.status, ReviewStatus::Pending);
    assert_eq!(revised.rating, 2);
    assert_eq!(revised.title, None);
    assert_eq!(revised.body, "Chipped after a week.");
    let photos: Vec<&str> = revised
        .photos
        .iter()
        .map(|photo| photo.id.as_str())
        .collect();
    assert_eq!(photos, ["photo-2", "photo-3"]);
    // What the order gave stays.
    assert_eq!(revised.author_name, "Ada L.");
    assert_eq!(revised.variant_label.as_deref(), Some("Beige"));

    // The same words again write nothing.
    let version = revised.aggregate_version().unwrap();
    let mut same = revision("order-1", MUG, &["photo-2", "photo-3"]);
    same.photos.clear();
    let unchanged = reviews.revise_review(SHOPPER, same).await.unwrap();
    assert_eq!(unchanged.aggregate_version().unwrap(), version);
}

#[tokio::test]
async fn a_revision_is_checked_and_ends_with_the_merchant() {
    let reviews = command().await;
    paid_order(&reviews, "order-1").await;
    let review = submit(&reviews, "order-1", MUG, 4).await;

    let refused = |cmd: ReviseReview| {
        let reviews = &reviews;
        async move { reviews.revise_review(SHOPPER, cmd).await.unwrap_err() }
    };
    assert!(matches!(
        refused(revision("order-1", TEA, &[])).await,
        ReviewError::NotFound
    ));
    assert!(matches!(
        refused(revision("order-1", MUG, &["photo-9"])).await,
        ReviewError::PhotoNotFound
    ));
    let mut crowded = revision("order-1", MUG, &["photo-1", "photo-2"]);
    crowded.photos.push(photo("photo-4"));
    assert!(matches!(refused(crowded).await, ReviewError::TooManyPhotos));
    let mut short = revision("order-1", MUG, &[]);
    short.body = "Meh".to_owned();
    assert!(matches!(refused(short).await, ReviewError::InvalidBody));

    reviews.publish_review(ALICE, &review.id).await.unwrap();
    assert!(matches!(
        refused(revision("order-1", MUG, &[])).await,
        ReviewError::NotPending
    ));

    let tea = submit(&reviews, "order-1", TEA, 5).await;
    reviews.reject_review(ALICE, &tea.id, "").await.unwrap();
    assert!(matches!(
        refused(revision("order-1", TEA, &[])).await,
        ReviewError::NotPending
    ));
}
