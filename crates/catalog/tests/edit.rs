mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_catalog::{CatalogError, DeliveryDays, Price};

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

#[tokio::test]
async fn the_delivery_time_is_a_range_of_days_or_the_shops_default() {
    let cmd = command().await;
    let product = create_product(&cmd, "Lamp").await;
    let id = product.id.as_str();
    assert_eq!(product.delivery_days, None);

    let days = DeliveryDays::new(8, 15).unwrap();
    cmd.set_product_delivery_days(ALICE, id, Some(days))
        .await
        .unwrap();
    let set = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(set.delivery_days, Some(days));

    // The same days write nothing.
    cmd.set_product_delivery_days(ALICE, id, Some(days))
        .await
        .unwrap();
    let again = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(
        again.aggregate_version().unwrap(),
        set.aggregate_version().unwrap()
    );

    let backwards = DeliveryDays { min: 15, max: 8 };
    let err = cmd
        .set_product_delivery_days(ALICE, id, Some(backwards))
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDeliveryDays), "{err}");

    cmd.set_product_delivery_days(ALICE, id, None)
        .await
        .unwrap();
    let cleared = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(cleared.delivery_days, None);

    assert_eq!(cmd.load_default_delivery_days().await.unwrap(), None);
    cmd.set_default_delivery_days(ALICE, Some(days))
        .await
        .unwrap();
    cmd.set_default_delivery_days(ALICE, Some(days))
        .await
        .unwrap();
    assert_eq!(cmd.load_default_delivery_days().await.unwrap(), Some(days));
    cmd.set_default_delivery_days(ALICE, None).await.unwrap();
    assert_eq!(cmd.load_default_delivery_days().await.unwrap(), None);
}

#[test]
fn delivery_days_read_from_a_form_and_make_a_window() {
    let days = |min, max| DeliveryDays::new(min, max).unwrap();
    assert_eq!(DeliveryDays::parse("", " ").unwrap(), None);
    assert_eq!(DeliveryDays::parse("3", "7").unwrap(), Some(days(3, 7)));
    assert_eq!(DeliveryDays::parse("5", "").unwrap(), Some(days(5, 5)));
    assert_eq!(DeliveryDays::parse("", "9").unwrap(), Some(days(9, 9)));
    for (min, max) in [("0", "3"), ("7", "3"), ("1", "366"), ("two", "3")] {
        assert!(
            matches!(
                DeliveryDays::parse(min, max),
                Err(CatalogError::InvalidDeliveryDays)
            ),
            "{min}–{max}"
        );
    }
    assert_eq!(
        DeliveryDays::window([Some(days(3, 5)), None, Some(days(8, 15))]),
        Some(days(8, 15))
    );
    assert_eq!(
        DeliveryDays::window([Some(days(10, 12)), Some(days(8, 15))]),
        Some(days(10, 15))
    );
    assert_eq!(DeliveryDays::window([None, None]), None);
}
