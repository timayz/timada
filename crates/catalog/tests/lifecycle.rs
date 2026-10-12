mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_catalog::{CatalogError, ProductStatus};

#[tokio::test]
async fn publishing_needs_a_price_and_repeats_as_a_no_op() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;
    let id = product.id.as_str();
    assert!(matches!(
        cmd.publish_product(ALICE, id).await,
        Err(CatalogError::NotPriced)
    ));
    cmd.unpublish_product(ALICE, id).await.unwrap();

    cmd.price_product(ALICE, id, 1250, "EUR").await.unwrap();
    cmd.publish_product(ALICE, id).await.unwrap();
    cmd.publish_product(ALICE, id).await.unwrap();
    let published = cmd.load_product(id).await.unwrap().unwrap();
    assert!(published.is_published());
    assert_eq!(published.aggregate_version().unwrap(), 3);

    cmd.unpublish_product(ALICE, id).await.unwrap();
    cmd.unpublish_product(ALICE, id).await.unwrap();
    let draft = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(draft.status, ProductStatus::Draft);
    assert_eq!(draft.aggregate_version().unwrap(), 4);
}

#[tokio::test]
async fn archiving_frees_the_slug_for_a_new_product() {
    let cmd = command().await;
    let first = create_product(&cmd, "Blue Mug").await;
    cmd.archive_product(ALICE, &first.id).await.unwrap();
    assert!(
        cmd.load_product(&first.id)
            .await
            .unwrap()
            .unwrap()
            .is_archived()
    );
    assert!(
        cmd.load_product_by_slug("blue-mug")
            .await
            .unwrap()
            .is_none(),
        "the slug points nowhere once archived"
    );

    let second = create_product(&cmd, "Blue Mug").await;
    assert_eq!(second.slug, "blue-mug", "the plain slug is free again");
    assert_eq!(
        cmd.load_product_by_slug("blue-mug")
            .await
            .unwrap()
            .unwrap()
            .id,
        second.id
    );
}
