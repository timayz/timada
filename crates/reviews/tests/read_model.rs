mod common;

use timada_core::db::Db;

use common::*;
use timada_reviews::aggregator::Submitted;
use timada_reviews::{
    DEFAULT_REQUEST_DELAY_DAYS, REQUEST_WINDOW_SECS, ReviewFilter, ReviewStatus, count_reviews,
    product_rating, read_model_subscription, review_id, review_request_mailed_at,
    review_requests_due, reviews_of_tenant,
};

const DAY: u64 = 24 * 60 * 60;

async fn catch_up(executor: &evento::Turso, pool: &Db) {
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn reviews_are_listed_and_products_rated_from_published_ones() {
    let (executor, pool) = executor_with_pool().await;
    let reviews = timada_reviews::Command::new(executor.clone(), TENANT);
    for order in ["order-1", "order-2", "order-3"] {
        paid_order(&executor, order).await;
    }
    let one = submit(&reviews, "order-1", MUG, 5).await.id;
    let two = submit(&reviews, "order-2", MUG, 4).await.id;
    let three = submit(&reviews, "order-3", MUG, 1).await.id;
    submit(&reviews, "order-1", TEA, 3).await;

    catch_up(&executor, &pool).await;
    let pending = ReviewFilter {
        status: Some(ReviewStatus::Pending),
        ..Default::default()
    };
    assert_eq!(count_reviews(&pool, TENANT, &pending).await.unwrap(), 4);
    assert_eq!(
        product_rating(&pool, TENANT, MUG).await.unwrap().average(),
        None
    );
    let rows = reviews_of_tenant(&pool, TENANT, &pending, 10, 0)
        .await
        .unwrap();
    let row = rows.iter().find(|row| row.id == one).unwrap();
    assert_eq!(row.order_reference, "REF-order-1");
    assert_eq!(row.product_name, "Mug");
    assert_eq!(row.author_name, "Ada L.");
    assert_eq!(row.photo_count, 2);

    reviews.publish_review(ALICE, &one).await.unwrap();
    reviews.publish_review(ALICE, &two).await.unwrap();
    reviews.publish_review(ALICE, &three).await.unwrap();
    reviews.reject_review(ALICE, &three, "rude").await.unwrap();
    reviews
        .remove_review_photo(ALICE, &two, "photo-1")
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    catch_up(&executor, &pool).await;

    let rating = product_rating(&pool, TENANT, MUG).await.unwrap();
    assert_eq!(rating.review_count, 2);
    assert_eq!(rating.average(), Some(4.5));
    let mug = ReviewFilter {
        product_id: Some(MUG.to_owned()),
        ..Default::default()
    };
    assert_eq!(count_reviews(&pool, TENANT, &mug).await.unwrap(), 3);
    let five = ReviewFilter {
        rating: Some(5),
        ..Default::default()
    };
    let rows = reviews_of_tenant(&pool, TENANT, &five, 10, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "published");
    let two_row = reviews_of_tenant(&pool, TENANT, &mug, 10, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == two)
        .unwrap();
    assert_eq!(two_row.photo_count, 1);
    assert_eq!(
        count_reviews(&pool, "other", &ReviewFilter::default())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn a_revised_review_is_listed_as_it_now_reads() {
    let (executor, pool) = executor_with_pool().await;
    let reviews = timada_reviews::Command::new(executor.clone(), TENANT);
    paid_order(&executor, "order-1").await;
    let id = submit(&reviews, "order-1", MUG, 5).await.id;
    catch_up(&executor, &pool).await;

    reviews
        .revise_review(
            SHOPPER,
            timada_reviews::ReviseReview {
                order_id: "order-1".to_owned(),
                product_id: MUG.to_owned(),
                rating: 2,
                title: Some("Chipped".to_owned()),
                body: "Chipped after a week.".to_owned(),
                keep: vec!["photo-1".to_owned()],
                photos: vec![],
            },
        )
        .await
        .unwrap();
    catch_up(&executor, &pool).await;

    let rows = reviews_of_tenant(&pool, TENANT, &ReviewFilter::default(), 10, 0)
        .await
        .unwrap();
    let row = rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row.rating, 2);
    assert_eq!(row.title.as_deref(), Some("Chipped"));
    assert_eq!(row.body, "Chipped after a week.");
    assert_eq!(row.photo_count, 1);
}

/// The order's page reads when its review request was mailed from the read
/// model.
#[tokio::test]
async fn a_mailed_review_request_is_found_by_its_order() {
    let (executor, pool) = executor_with_pool().await;
    let reviews = timada_reviews::Command::new(executor.clone(), TENANT);
    assert!(
        reviews
            .claim_review_request("order-1", "ada@example.com", SHOPPER)
            .await
            .unwrap()
    );
    for _ in 0..2 {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
    }

    let mailed_at = timada_reviews::review_request_mailed_at(&pool, TENANT, "order-1")
        .await
        .unwrap();
    assert!(mailed_at.is_some_and(|at| at > 0));
    assert_eq!(
        timada_reviews::review_request_mailed_at(&pool, TENANT, "order-2")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        timada_reviews::review_request_mailed_at(&pool, "other", "order-1")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn the_row_says_what_the_review_says_of_its_order_with_no_order_in_the_store() {
    let (executor, pool) = executor_with_pool().await;
    // No order stream anywhere: the review carries its reference and name.
    let id = review_id("order-9", MUG);
    evento::append(&id)
        .original_version(0)
        .event(&Submitted {
            order_id: "order-9".to_owned(),
            order_reference: "REF-order-9".to_owned(),
            product_id: MUG.to_owned(),
            product_name: "Mug".to_owned(),
            variant_label: Some("Beige".to_owned()),
            rating: 4,
            title: None,
            body: "Holds my tea all day long.".to_owned(),
            author_name: "Ada L.".to_owned(),
            photos: vec![],
            submitted_by: SHOPPER.to_owned(),
        })
        .tenant(TENANT)
        .commit(&executor)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;

    let rows = reviews_of_tenant(&pool, TENANT, &ReviewFilter::default(), 10, 0)
        .await
        .unwrap();
    let row = rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row.order_reference, "REF-order-9");
    assert_eq!(row.product_name, "Mug");
}

/// The orders whose review request is due at `now`, by id, in the order a
/// walk reads them.
async fn due(pool: &Db, now: u64) -> Vec<String> {
    review_requests_due(pool, now, None, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|request| request.order_id)
        .collect()
}

#[tokio::test]
async fn a_fulfilled_order_falls_due_as_the_shops_policy_says() {
    let (executor, pool) = executor_with_pool().await;
    let reviews = timada_reviews::Command::new(executor.clone(), TENANT);
    paid_order(&executor, "order-1").await;
    let left = fulfil(&executor, "order-1").await;
    catch_up(&executor, &pool).await;
    let week = left + u64::from(DEFAULT_REQUEST_DELAY_DAYS) * DAY;
    assert!(due(&pool, week - 1).await.is_empty(), "not yet");
    let listed = review_requests_due(&pool, week, None, 10).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].order_id, "order-1");
    assert_eq!(listed[0].tenant_id, TENANT);
    assert_eq!(listed[0].fulfilled_at, left as i64);
    assert_eq!(listed[0].due_at, week as i64);
    assert!(
        due(&pool, week + REQUEST_WINDOW_SECS).await.is_empty(),
        "too late"
    );

    // The merchant waits longer: due later.
    reviews.set_review_requests(ALICE, Some(14)).await.unwrap();
    catch_up(&executor, &pool).await;
    assert!(due(&pool, week).await.is_empty());
    assert_eq!(due(&pool, left + 14 * DAY).await, ["order-1"]);

    // Stops asking: nothing is due, nor an order that leaves meanwhile.
    reviews.set_review_requests(ALICE, None).await.unwrap();
    paid_order(&executor, "order-2").await;
    fulfil(&executor, "order-2").await;
    catch_up(&executor, &pool).await;
    assert!(due(&pool, left + 14 * DAY).await.is_empty());
    assert!(due(&pool, left + DAY).await.is_empty());

    // Asks again, at once: both are due.
    reviews.set_review_requests(ALICE, Some(0)).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(due(&pool, left + DAY).await, ["order-1", "order-2"]);

    // Mailed: due no more, whatever the policy says next, and the order's
    // page says when.
    assert!(
        reviews
            .claim_review_request("order-1", "ada@example.com", SHOPPER)
            .await
            .unwrap()
    );
    catch_up(&executor, &pool).await;
    assert_eq!(due(&pool, left + DAY).await, ["order-2"]);
    reviews.set_review_requests(ALICE, Some(1)).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(due(&pool, left + 2 * DAY).await, ["order-2"]);
    assert!(
        review_request_mailed_at(&pool, TENANT, "order-1")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        review_request_mailed_at(&pool, TENANT, "order-2")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn the_requests_due_are_walked_page_by_page() {
    let (executor, pool) = executor_with_pool().await;
    let mut left = 0;
    for n in 0..5 {
        let order_id = format!("order-{n}");
        paid_order(&executor, &order_id).await;
        left = fulfil(&executor, &order_id).await;
    }
    catch_up(&executor, &pool).await;
    let now = left + u64::from(DEFAULT_REQUEST_DELAY_DAYS) * DAY + 60;
    let all = review_requests_due(&pool, now, None, 100).await.unwrap();
    assert_eq!(all.len(), 5);
    let mut walked = Vec::new();
    let mut after = None;
    loop {
        let page = review_requests_due(&pool, now, after.as_ref(), 2)
            .await
            .unwrap();
        let Some(last) = page.last().cloned() else {
            break;
        };
        assert!(page.len() <= 2);
        walked.extend(page);
        after = Some(last);
    }
    assert_eq!(walked, all);
}
