mod common;

use common::*;
use timada_catalog::{
    Command, ProductStatus, count_products, product_by_id, products_of_tenant,
    read_model_subscription,
};

const TENANT: &str = "tenant-acme";

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    read_model_subscription::<evento::Sqlite>(pool.clone(), TENANT)
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn products_are_listed_searched_counted_and_stay_idempotent() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let mug = create_product(&cmd, "Blue Mug").await;
    let plate = create_product(&cmd, "Plate").await;
    cmd.price_product(ALICE, &mug.id, 1250, "EUR")
        .await
        .unwrap();
    cmd.describe_product(ALICE, &mug.id, "Holds 50 cl.")
        .await
        .unwrap();
    cmd.publish_product(ALICE, &mug.id).await.unwrap();

    catch_up(&executor, &pool).await;
    catch_up(&executor, &pool).await;

    assert_eq!(count_products(&pool, TENANT, None).await.unwrap(), 2);
    assert_eq!(count_products(&pool, "other", None).await.unwrap(), 0);
    let rows = products_of_tenant(&pool, TENANT, None, 0, 50)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    // Both were changed within the same second, so the order between them is
    // the id tiebreak; look them up instead.
    let mug_row = rows.iter().find(|r| r.id == mug.id).unwrap();
    assert_eq!(mug_row.name, "Blue Mug");
    assert_eq!(mug_row.slug, "blue-mug");
    assert_eq!(mug_row.description, "Holds 50 cl.");
    assert_eq!(mug_row.price().unwrap().to_string(), "12.50 EUR");
    assert_eq!(mug_row.status(), ProductStatus::Published);
    assert_eq!(mug_row.created_by, ALICE);
    let plate_row = rows.iter().find(|r| r.id == plate.id).unwrap();
    assert_eq!(plate_row.price(), None);
    assert_eq!(plate_row.status(), ProductStatus::Draft);

    let found = products_of_tenant(&pool, TENANT, Some("MUG"), 0, 50)
        .await
        .unwrap();
    assert_eq!(found.len(), 1, "search is case-insensitive");
    assert_eq!(count_products(&pool, TENANT, Some("%")).await.unwrap(), 0);
    assert_eq!(
        product_by_id(&pool, TENANT, &plate.id)
            .await
            .unwrap()
            .unwrap()
            .name,
        "Plate"
    );
    assert!(
        product_by_id(&pool, "other", &plate.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn an_archived_product_leaves_its_slug_to_the_next_one() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let first = create_product(&cmd, "Blue Mug").await;
    cmd.archive_product(ALICE, &first.id).await.unwrap();
    let second = create_product(&cmd, "Blue Mug").await;
    catch_up(&executor, &pool).await;

    let rows = products_of_tenant(&pool, TENANT, Some("blue-mug"), 0, 50)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "both rows keep the slug, one archived");
    assert_eq!(rows[0].id, second.id);
    assert_eq!(rows[0].status(), ProductStatus::Draft);
    assert_eq!(rows[1].id, first.id);
    assert_eq!(rows[1].status(), ProductStatus::Archived);
}
