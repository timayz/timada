mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_catalog::{CatalogError, Price};

#[tokio::test]
async fn edits_update_the_state_and_repeat_as_no_ops() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;
    let id = product.id.as_str();

    cmd.rename_product(ALICE, id, " Big Blue Mug ")
        .await
        .unwrap();
    cmd.describe_product(ALICE, id, "Holds 50 cl.")
        .await
        .unwrap();
    cmd.price_product(ALICE, id, 1250, "eur").await.unwrap();
    let edited = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(edited.name, "Big Blue Mug");
    assert_eq!(edited.description, "Holds 50 cl.");
    assert_eq!(
        edited.price,
        Some(Price {
            amount_cents: 1250,
            currency: "EUR".into()
        })
    );
    assert_eq!(edited.price.as_ref().unwrap().to_string(), "12.50 €");
    assert_eq!(edited.aggregate_version().unwrap(), 4);

    // The same values again write nothing.
    cmd.rename_product(ALICE, id, "Big Blue Mug").await.unwrap();
    cmd.describe_product(ALICE, id, "Holds 50 cl.")
        .await
        .unwrap();
    cmd.price_product(ALICE, id, 1250, "EUR").await.unwrap();
    let again = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version().unwrap(), 4);
    assert_eq!(again, edited);
}

#[tokio::test]
async fn renaming_moves_the_product_to_the_slug_of_its_new_name() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;

    cmd.rename_product(ALICE, &mug.id, "Tea Cup").await.unwrap();
    let renamed = cmd.load_product(&mug.id).await.unwrap().unwrap();
    assert_eq!(renamed.slug, "tea-cup");
    let found = cmd.load_product_by_slug("tea-cup").await.unwrap().unwrap();
    assert_eq!(found.id, mug.id);
    assert!(
        cmd.load_product_by_slug("blue-mug")
            .await
            .unwrap()
            .is_none(),
        "the old slug is free"
    );

    // Another product can have it; the first, coming back to the name, gets
    // a suffix.
    let other = create_product(&cmd, "Blue Mug").await;
    assert_eq!(other.slug, "blue-mug");
    cmd.rename_product(ALICE, &mug.id, "Blue Mug")
        .await
        .unwrap();
    let suffixed = cmd.load_product(&mug.id).await.unwrap().unwrap();
    assert_ne!(suffixed.slug, "blue-mug");
    assert!(suffixed.slug.starts_with("blue-mug-"), "{}", suffixed.slug);
    assert!(
        cmd.load_product_by_slug("tea-cup").await.unwrap().is_none(),
        "the slug it left is free"
    );

    // The old slug comes back to a product that returns to it.
    cmd.rename_product(ALICE, &other.id, "Plate").await.unwrap();
    cmd.rename_product(ALICE, &other.id, "Blue Mug")
        .await
        .unwrap();
    let back = cmd.load_product(&other.id).await.unwrap().unwrap();
    assert_eq!(back.slug, "blue-mug");
}

#[tokio::test]
async fn renaming_keeps_the_slug_when_the_name_gives_the_same_one() {
    let cmd = command().await;
    let first = create_product(&cmd, "Blue Mug").await;
    let suffixed = create_product(&cmd, "Blue Mug").await;
    assert_ne!(suffixed.slug, first.slug);

    // Not a new suffix for a new capital.
    cmd.rename_product(ALICE, &suffixed.id, "BLUE MUG")
        .await
        .unwrap();
    let renamed = cmd.load_product(&suffixed.id).await.unwrap().unwrap();
    assert_eq!(renamed.name, "BLUE MUG");
    assert_eq!(renamed.slug, suffixed.slug);
    let found = cmd
        .load_product_by_slug(&suffixed.slug)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, suffixed.id);
}

#[tokio::test]
async fn edits_are_validated() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;
    let id = product.id.as_str();
    assert!(matches!(
        cmd.rename_product(ALICE, id, "").await,
        Err(CatalogError::InvalidName)
    ));
    assert!(matches!(
        cmd.describe_product(ALICE, id, &"d".repeat(4001)).await,
        Err(CatalogError::InvalidDescription)
    ));
    assert!(matches!(
        cmd.price_product(ALICE, id, 100, "XYZ").await,
        Err(CatalogError::InvalidCurrency)
    ));
    assert!(matches!(
        cmd.price_product(ALICE, id, u64::MAX, "EUR").await,
        Err(CatalogError::InvalidAmount)
    ));
    assert!(matches!(
        cmd.rename_product(ALICE, "missing", "Name").await,
        Err(CatalogError::NotFound)
    ));
}

#[tokio::test]
async fn an_archived_product_refuses_edits() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;
    let id = product.id.as_str();
    cmd.archive_product(ALICE, id).await.unwrap();
    for result in [
        cmd.rename_product(ALICE, id, "New").await,
        cmd.describe_product(ALICE, id, "New").await,
        cmd.price_product(ALICE, id, 100, "EUR").await,
        cmd.publish_product(ALICE, id).await,
        cmd.unpublish_product(ALICE, id).await,
    ] {
        assert!(matches!(result, Err(CatalogError::Archived)), "{result:?}");
    }
    cmd.archive_product(ALICE, id).await.unwrap();
    assert_eq!(
        cmd.load_product(id)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version()
            .unwrap(),
        2,
        "archiving twice writes once"
    );
}

#[tokio::test]
async fn the_tax_category_is_a_known_one_or_standard() {
    let cmd = command().await;
    let product = create_product(&cmd, "Book").await;
    let id = product.id.as_str();
    assert_eq!(product.tax_category(), "standard");

    cmd.set_product_tax_category(ALICE, id, Some("books"))
        .await
        .unwrap();
    let book = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(book.tax_category(), "books");

    // The same category writes nothing.
    cmd.set_product_tax_category(ALICE, id, Some("books"))
        .await
        .unwrap();
    let again = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(
        again.aggregate_version().unwrap(),
        book.aggregate_version().unwrap()
    );

    let err = cmd
        .set_product_tax_category(ALICE, id, Some("cars"))
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::InvalidTaxCategory), "{err}");

    cmd.set_product_tax_category(ALICE, id, Some("standard"))
        .await
        .unwrap();
    let standard = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(standard.tax_category, None);
}
