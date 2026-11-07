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

#[tokio::test]
async fn deleting_hides_the_product_and_frees_its_slug() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    cmd.price_product(ALICE, &mug.id, 1250, "EUR")
        .await
        .unwrap();
    cmd.publish_product(ALICE, &mug.id).await.unwrap();

    let gone = cmd.delete_product(ALICE, &mug.id).await.unwrap();
    assert_eq!(gone.id, mug.id, "what it was comes back, for the clean-up");
    assert!(
        cmd.load_product(&mug.id).await.unwrap().is_none(),
        "gone from every page"
    );
    let state = cmd.load_product_any(&mug.id).await.unwrap().unwrap();
    assert!(state.is_deleted());
    assert!(
        state.is_archived(),
        "deleted counts as archived to everyone else"
    );
    assert!(!state.is_published());
    assert!(matches!(
        cmd.delete_product(ALICE, &mug.id).await,
        Err(CatalogError::NotFound)
    ));
    assert!(matches!(
        cmd.rename_product(ALICE, &mug.id, "Red Mug").await,
        Err(CatalogError::NotFound)
    ));
    assert!(
        cmd.load_product_by_slug("blue-mug")
            .await
            .unwrap()
            .is_none()
    );

    let second = create_product(&cmd, "Blue Mug").await;
    assert_eq!(second.slug, "blue-mug", "the plain slug is free again");
}

#[tokio::test]
async fn an_archived_product_can_still_be_deleted() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    cmd.archive_product(ALICE, &mug.id).await.unwrap();
    cmd.delete_product(ALICE, &mug.id).await.unwrap();
    assert!(cmd.load_product(&mug.id).await.unwrap().is_none());
    let second = create_product(&cmd, "Blue Mug").await;
    assert_eq!(
        second.slug, "blue-mug",
        "the slug was freed once, by archiving, and stays free"
    );
}

#[tokio::test]
async fn an_ordered_product_is_never_deleted() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    assert!(cmd.load_ordered(&mug.id).await.unwrap().is_none());
    cmd.mark_ordered(ALICE, &mug.id, "order-1").await.unwrap();
    // Told again, and by a later order: the first order stays the mark.
    cmd.mark_ordered(ALICE, &mug.id, "order-1").await.unwrap();
    cmd.mark_ordered(ALICE, &mug.id, "order-2").await.unwrap();
    let ordered = cmd.load_ordered(&mug.id).await.unwrap().unwrap();
    assert_eq!(ordered.order_id, "order-1");
    assert_eq!(ordered.aggregate_version().unwrap(), 1, "written once");

    assert!(matches!(
        cmd.delete_product(ALICE, &mug.id).await,
        Err(CatalogError::Ordered)
    ));
    assert!(cmd.load_product(&mug.id).await.unwrap().is_some());
    // It may still be archived.
    cmd.archive_product(ALICE, &mug.id).await.unwrap();
    assert!(matches!(
        cmd.delete_product(ALICE, &mug.id).await,
        Err(CatalogError::Ordered)
    ));
}
