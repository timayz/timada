mod common;

use common::*;
use timada_catalog::{
    Command, ProductFilter, ProductStatus, categories_of_product, categories_of_tenant,
    category_by_id, count_products, live_child_count, product_by_id, products_of_tenant,
    read_model_subscription, tree_of, upsert_product,
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

    let all = ProductFilter::default();
    assert_eq!(count_products(&pool, TENANT, &all).await.unwrap(), 2);
    assert_eq!(count_products(&pool, "other", &all).await.unwrap(), 0);
    let rows = products_of_tenant(&pool, TENANT, &all, 0, 50)
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

    let found = products_of_tenant(&pool, TENANT, &all.clone().search(Some("MUG")), 0, 50)
        .await
        .unwrap();
    assert_eq!(found.len(), 1, "search is case-insensitive");
    assert_eq!(
        count_products(&pool, TENANT, &all.clone().search(Some("%")))
            .await
            .unwrap(),
        0
    );
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

    let rows = products_of_tenant(
        &pool,
        TENANT,
        &ProductFilter::default().search(Some("blue-mug")),
        0,
        50,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "both rows keep the slug, one archived");
    assert_eq!(rows[0].id, second.id);
    assert_eq!(rows[0].status(), ProductStatus::Draft);
    assert_eq!(rows[1].id, first.id);
    assert_eq!(rows[1].status(), ProductStatus::Archived);
}

#[tokio::test]
async fn the_category_tree_carries_its_product_counts() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let bedding = create_category(&cmd, "Bedding").await;
    let mats = create_child(&cmd, "Mats", &bedding.id).await;
    let toys = create_category(&cmd, "Toys").await;
    cmd.position_category(ALICE, &toys.id, 1).await.unwrap();
    let mat = create_product(&cmd, "Cooling mat").await;
    let ball = create_product(&cmd, "Ball").await;
    cmd.categorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    cmd.categorize_product(ALICE, &mat.id, &mats.id)
        .await
        .unwrap();
    cmd.categorize_product(ALICE, &ball.id, &toys.id)
        .await
        .unwrap();

    catch_up(&executor, &pool).await;
    catch_up(&executor, &pool).await;

    let rows = categories_of_tenant(&pool, TENANT).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        categories_of_tenant(&pool, "other")
            .await
            .unwrap()
            .is_empty(),
        "rows are the tenant's own"
    );
    let tree = tree_of(rows);
    assert_eq!(
        tree.iter()
            .map(|n| n.item.name.as_str())
            .collect::<Vec<_>>(),
        ["Bedding", "Toys"],
        "position orders the top level"
    );
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].children[0].item.name, "Mats");
    assert_eq!(tree[0].item.product_count, 1);
    assert_eq!(tree[0].children[0].item.product_count, 1);
    assert_eq!(tree[1].item.product_count, 1);
    assert_eq!(
        live_child_count(&pool, TENANT, &bedding.id).await.unwrap(),
        1
    );
    assert_eq!(live_child_count(&pool, TENANT, &toys.id).await.unwrap(), 0);

    // The filter takes a category, and membership rows follow the product.
    let in_bedding = ProductFilter::default().category(Some(&bedding.id));
    assert_eq!(count_products(&pool, TENANT, &in_bedding).await.unwrap(), 1);
    assert_eq!(
        products_of_tenant(&pool, TENANT, &in_bedding, 0, 50)
            .await
            .unwrap()[0]
            .id,
        mat.id
    );
    assert_eq!(
        categories_of_product(&pool, TENANT, &mat.id)
            .await
            .unwrap()
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Bedding", "Mats"]
    );

    cmd.uncategorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        count_products(&pool, TENANT, &in_bedding).await.unwrap(),
        0,
        "leaving a category removes the row"
    );
    assert_eq!(
        category_by_id(&pool, TENANT, &bedding.id)
            .await
            .unwrap()
            .unwrap()
            .product_count,
        0
    );
}

#[tokio::test]
async fn an_archived_category_leaves_the_tree_but_keeps_its_memberships() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let bedding = create_category(&cmd, "Bedding").await;
    let mat = create_product(&cmd, "Cooling mat").await;
    cmd.categorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    cmd.archive_category(ALICE, &bedding.id).await.unwrap();
    catch_up(&executor, &pool).await;

    assert!(
        categories_of_tenant(&pool, TENANT)
            .await
            .unwrap()
            .is_empty(),
        "an archived category is not listed"
    );
    let row = category_by_id(&pool, TENANT, &bedding.id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.archived, "it is still there by id");
    let memberships = categories_of_product(&pool, TENANT, &mat.id).await.unwrap();
    assert_eq!(memberships.len(), 1);
    assert!(
        memberships[0].archived,
        "so the product page can show it as archived"
    );
}

#[tokio::test]
async fn an_archived_product_drops_out_of_its_categories_count() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let bedding = create_category(&cmd, "Bedding").await;
    let mat = create_product(&cmd, "Cooling mat").await;
    cmd.categorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        category_by_id(&pool, TENANT, &bedding.id)
            .await
            .unwrap()
            .unwrap()
            .product_count,
        1
    );

    cmd.archive_product(ALICE, &mat.id).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        category_by_id(&pool, TENANT, &bedding.id)
            .await
            .unwrap()
            .unwrap()
            .product_count,
        0,
        "an archived product is not counted"
    );
}

#[tokio::test]
async fn the_stored_counts_follow_a_move() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let bedding = create_category(&cmd, "Bedding").await;
    let mats = create_child(&cmd, "Mats", &bedding.id).await;
    let toys = create_category(&cmd, "Toys").await;
    let mat = create_product(&cmd, "Cooling mat").await;
    cmd.categorize_product(ALICE, &mat.id, &bedding.id)
        .await
        .unwrap();
    cmd.categorize_product(ALICE, &mat.id, &mats.id)
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    let row = |id: String| {
        let pool = pool.clone();
        async move {
            let row = category_by_id(&pool, TENANT, &id).await.unwrap().unwrap();
            (row.product_count, row.subtree_count)
        }
    };
    assert_eq!(row(bedding.id.clone()).await, (1, 1), "counted once");
    assert_eq!(row(mats.id.clone()).await, (1, 1));
    assert_eq!(row(toys.id.clone()).await, (0, 0));

    cmd.move_category(ALICE, &mats.id, Some(&toys.id))
        .await
        .unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(row(bedding.id.clone()).await, (1, 1));
    assert_eq!(
        row(toys.id.clone()).await,
        (0, 1),
        "the new parent lists what came with it"
    );

    cmd.archive_product(ALICE, &mat.id).await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(row(bedding.id).await, (0, 0));
    assert_eq!(row(toys.id).await, (0, 0));
}

#[tokio::test]
async fn a_search_reads_the_index_of_its_own_tenant() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let mug = create_product(&cmd, "Café Mug").await;
    create_product(&cmd, "Plate").await;
    catch_up(&executor, &pool).await;
    // A second tenant with a product of the same name.
    upsert_product(&pool, "tenant-zeta", &mug).await.unwrap();
    let search = |q: &'static str| ProductFilter::default().search(Some(q));

    assert_eq!(
        count_products(&pool, TENANT, &search("cafe"))
            .await
            .unwrap(),
        1,
        "one tenant's match, accents ignored"
    );
    assert_eq!(
        count_products(&pool, "tenant-zeta", &search("CAFÉ"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        count_products(&pool, TENANT, &search("é m")).await.unwrap(),
        1,
        "a phrase matches across a space"
    );
    assert_eq!(
        count_products(&pool, TENANT, &search("ug")).await.unwrap(),
        1,
        "shorter than the index cuts, still found"
    );

    cmd.rename_product(ALICE, &mug.id, "Tea Cup").await.unwrap();
    catch_up(&executor, &pool).await;
    assert_eq!(
        count_products(&pool, TENANT, &search("é m")).await.unwrap(),
        0,
        "the old name is out of the index (the slug keeps \"cafe\")"
    );
    let found = products_of_tenant(&pool, TENANT, &search("tea c"), 0, 50)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, mug.id);
    assert_eq!(
        count_products(&pool, "tenant-zeta", &search("é m"))
            .await
            .unwrap(),
        1,
        "the other tenant's product kept its name"
    );
}
