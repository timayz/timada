mod common;

use common::*;
use timada_catalog::{CatalogError, MAX_CATEGORIES_PER_PRODUCT};

#[tokio::test]
async fn a_product_joins_and_leaves_categories() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cooling mat").await;
    let bedding = create_category(&cmd, "Bedding").await;
    let mats = create_child(&cmd, "Mats", &bedding.id).await;

    cmd.categorize_product(ALICE, &product.id, &bedding.id)
        .await
        .unwrap();
    cmd.categorize_product(ALICE, &product.id, &mats.id)
        .await
        .unwrap();
    let in_both = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(in_both.categories.len(), 2);
    assert!(in_both.categories.contains(&bedding.id));
    assert!(in_both.categories.contains(&mats.id));

    let version = in_both.aggregate_version;
    cmd.categorize_product(ALICE, &product.id, &mats.id)
        .await
        .unwrap();
    assert_eq!(
        cmd.load_product(&product.id)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        version,
        "joining twice writes nothing"
    );

    cmd.uncategorize_product(ALICE, &product.id, &bedding.id)
        .await
        .unwrap();
    let left = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(left.categories.iter().collect::<Vec<_>>(), [&mats.id]);
    cmd.uncategorize_product(ALICE, &product.id, &bedding.id)
        .await
        .expect("leaving twice is a no-op");
}

#[tokio::test]
async fn only_a_live_category_takes_products_but_leaving_one_always_works() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cooling mat").await;
    let bedding = create_category(&cmd, "Bedding").await;
    cmd.categorize_product(ALICE, &product.id, &bedding.id)
        .await
        .unwrap();
    cmd.archive_category(ALICE, &bedding.id).await.unwrap();

    let other = create_product(&cmd, "Basket").await;
    let archived = cmd.categorize_product(ALICE, &other.id, &bedding.id).await;
    assert!(
        matches!(archived, Err(CatalogError::CategoryArchived)),
        "{archived:?}"
    );
    let unknown = cmd.categorize_product(ALICE, &other.id, "nope").await;
    assert!(
        matches!(unknown, Err(CatalogError::CategoryNotFound)),
        "{unknown:?}"
    );

    assert!(
        cmd.load_product(&product.id)
            .await
            .unwrap()
            .unwrap()
            .categories
            .contains(&bedding.id),
        "an archived category keeps the products it had"
    );
    cmd.uncategorize_product(ALICE, &product.id, &bedding.id)
        .await
        .expect("and they can still leave it");
}

#[tokio::test]
async fn a_product_belongs_to_at_most_ten_categories() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cooling mat").await;
    for n in 0..MAX_CATEGORIES_PER_PRODUCT {
        let category = create_category(&cmd, &format!("Category {n}")).await;
        cmd.categorize_product(ALICE, &product.id, &category.id)
            .await
            .unwrap();
    }
    let one_more = create_category(&cmd, "One more").await;
    let err = cmd
        .categorize_product(ALICE, &product.id, &one_more.id)
        .await;
    assert!(
        matches!(err, Err(CatalogError::TooManyCategories)),
        "{err:?}"
    );
}

#[tokio::test]
async fn an_archived_product_takes_no_membership_changes() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cooling mat").await;
    let bedding = create_category(&cmd, "Bedding").await;
    cmd.archive_product(ALICE, &product.id).await.unwrap();
    for err in [
        cmd.categorize_product(ALICE, &product.id, &bedding.id)
            .await,
        cmd.uncategorize_product(ALICE, &product.id, &bedding.id)
            .await,
    ] {
        assert!(matches!(err, Err(CatalogError::Archived)), "{err:?}");
    }
}
