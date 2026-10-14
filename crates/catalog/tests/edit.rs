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
    assert_eq!(edited.price.as_ref().unwrap().to_string(), "12.50 EUR");
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
async fn the_vat_rate_is_a_french_one_or_the_default() {
    let cmd = command().await;
    let product = create_product(&cmd, "Book").await;
    let id = product.id.as_str();
    assert_eq!(product.vat_rate_bps, None);

    cmd.set_product_vat_rate(ALICE, id, Some(550))
        .await
        .unwrap();
    let reduced = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(reduced.vat_rate_bps, Some(550));

    // The same rate writes nothing.
    cmd.set_product_vat_rate(ALICE, id, Some(550))
        .await
        .unwrap();
    let again = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(
        again.aggregate_version().unwrap(),
        reduced.aggregate_version().unwrap()
    );

    let err = cmd
        .set_product_vat_rate(ALICE, id, Some(1900))
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::InvalidVatRate), "{err}");

    cmd.set_product_vat_rate(ALICE, id, None).await.unwrap();
    let default = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(default.vat_rate_bps, None);
}
