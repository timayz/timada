mod common;

use common::*;
use timada_reviews::{
    ReviewFilter, ReviewStatus, count_reviews, product_rating, read_model_subscription,
    reviews_of_tenant,
};

const TENANT: &str = "tenant-acme";

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    read_model_subscription::<evento::Sqlite>(pool.clone(), TENANT)
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn reviews_are_listed_and_products_rated_from_published_ones() {
    let (executor, pool) = executor_with_pool().await;
    let reviews = timada_reviews::Command(executor.clone());
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
