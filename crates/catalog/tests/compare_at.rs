mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_catalog::{CatalogError, Command, DefineVariant, OptionSelection, VariantPrice};

/// A published product at 20 €.
async fn on_sale(cmd: &Command<evento::Turso>) -> String {
    let product = create_product(cmd, "Blue Mug").await;
    cmd.price_product(ALICE, &product.id, 2000, "EUR")
        .await
        .unwrap();
    cmd.publish_product(ALICE, &product.id).await.unwrap();
    product.id
}

#[tokio::test]
async fn a_reduction_is_measured_from_a_price_the_product_was_on_sale_at() {
    let cmd = command().await;
    let id = on_sale(&cmd).await;
    let id = id.as_str();

    // On sale at one price only: nothing to compare it with.
    let alone = cmd.set_compare_at(ALICE, id, None, Some(2500)).await;
    assert!(matches!(alone, Err(CatalogError::NoEarlierPrice)));

    // Lowered to 15 €: it may say it was 20 €, no more, and not what it is.
    cmd.price_product(ALICE, id, 1500, "EUR").await.unwrap();
    let state = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(state.compare_at_ceiling(None), Some(2000));
    assert_eq!(state.compare_at(None), None);
    let above = cmd.set_compare_at(ALICE, id, None, Some(2500)).await;
    assert!(matches!(above, Err(CatalogError::CompareAtAboveEarlier)));
    let same = cmd.set_compare_at(ALICE, id, None, Some(1500)).await;
    assert!(matches!(same, Err(CatalogError::CompareAtNotAbove)));
    cmd.set_compare_at(ALICE, id, None, Some(2000))
        .await
        .unwrap();
    let reduced = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(reduced.compare_at(None), Some(2000));
    let version = reduced.aggregate_version().unwrap();
    cmd.set_compare_at(ALICE, id, None, Some(2000))
        .await
        .unwrap();
    let again = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version().unwrap(), version);

    // Lowered further, it is still down from 20 €.
    cmd.price_product(ALICE, id, 1200, "EUR").await.unwrap();
    let further = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(further.compare_at(None), Some(2000));

    // Back at 20 €, there is no reduction left, and none comes back when
    // the price drops again: that one is measured from other prices.
    cmd.price_product(ALICE, id, 2000, "EUR").await.unwrap();
    cmd.price_product(ALICE, id, 1500, "EUR").await.unwrap();
    let anew = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(anew.compare_at(None), None);
    assert_eq!(anew.compare_at_cents, None);
    assert_eq!(
        anew.compare_at_ceiling(None),
        Some(1200),
        "the lowest of the days before, not the last"
    );
    let high = cmd.set_compare_at(ALICE, id, None, Some(2000)).await;
    assert!(matches!(high, Err(CatalogError::CompareAtAboveEarlier)));
}

#[tokio::test]
async fn a_comparison_is_taken_back() {
    let cmd = command().await;
    let id = on_sale(&cmd).await;
    cmd.price_product(ALICE, &id, 1500, "EUR").await.unwrap();
    cmd.set_compare_at(ALICE, &id, None, Some(1800))
        .await
        .unwrap();
    cmd.set_compare_at(ALICE, &id, None, None).await.unwrap();
    let state = cmd.load_product(&id).await.unwrap().unwrap();
    assert_eq!(state.compare_at(None), None);
    let version = state.aggregate_version().unwrap();
    // Nothing to take back is a no-op.
    cmd.set_compare_at(ALICE, &id, None, None).await.unwrap();
    let again = cmd.load_product(&id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version().unwrap(), version);
}

#[tokio::test]
async fn a_draft_price_is_nothing_to_compare_with() {
    let cmd = command().await;
    let draft = create_product(&cmd, "Blue Mug").await;
    let id = draft.id.as_str();
    let unpriced = cmd.set_compare_at(ALICE, id, None, Some(2000)).await;
    assert!(matches!(unpriced, Err(CatalogError::NotPriced)));

    // 100 € that nobody could pay, then 15 €: no reduction to announce,
    // before or after it goes on sale.
    cmd.price_product(ALICE, id, 10_000, "EUR").await.unwrap();
    cmd.price_product(ALICE, id, 1500, "EUR").await.unwrap();
    let never = cmd.set_compare_at(ALICE, id, None, Some(10_000)).await;
    assert!(matches!(never, Err(CatalogError::NoEarlierPrice)));
    cmd.publish_product(ALICE, id).await.unwrap();
    let still = cmd.set_compare_at(ALICE, id, None, Some(10_000)).await;
    assert!(matches!(still, Err(CatalogError::NoEarlierPrice)));

    // Off sale and back at the same price, a reduction still stands.
    cmd.price_product(ALICE, id, 1200, "EUR").await.unwrap();
    cmd.set_compare_at(ALICE, id, None, Some(1500))
        .await
        .unwrap();
    cmd.unpublish_product(ALICE, id).await.unwrap();
    cmd.publish_product(ALICE, id).await.unwrap();
    let back = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(back.compare_at(None), Some(1500));
    assert_eq!(back.compare_at_ceiling(None), Some(1500));
}

#[tokio::test]
async fn each_variant_is_compared_with_its_own_prices() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cat Harness").await;
    let id = product.id.as_str();
    let size = cmd
        .define_option(ALICE, id, "Size", vec!["S".into(), "M".into()])
        .await
        .unwrap();
    let mut variants = Vec::new();
    for value in ["S", "M"] {
        let variant = cmd
            .define_variant(
                ALICE,
                id,
                DefineVariant {
                    selections: vec![OptionSelection {
                        option_id: size.clone(),
                        value: value.into(),
                    }],
                    sku: None,
                    amount_cents: 2290,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap();
        variants.push(variant);
    }
    let (small, medium) = (variants[0].as_str(), variants[1].as_str());
    cmd.publish_product(ALICE, id).await.unwrap();
    cmd.price_variant(ALICE, id, small, 1990, "EUR")
        .await
        .unwrap();

    cmd.set_compare_at(ALICE, id, Some(small), Some(2290))
        .await
        .unwrap();
    let other = cmd
        .set_compare_at(ALICE, id, Some(medium), Some(2500))
        .await;
    assert!(matches!(other, Err(CatalogError::NoEarlierPrice)));
    let unknown = cmd
        .set_compare_at(ALICE, id, Some("nope"), Some(2500))
        .await;
    assert!(matches!(unknown, Err(CatalogError::VariantNotFound)));
    let state = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(state.compare_at(Some(small)), Some(2290));
    assert_eq!(state.compare_at(Some(medium)), None);
    assert_eq!(state.compare_at(None), None);

    // A supplier's read prices several at once: the one that caught up
    // with its comparison loses it.
    cmd.price_variants(
        ALICE,
        id,
        &[
            VariantPrice {
                variant_id: small.to_owned(),
                amount_cents: 2400,
            },
            VariantPrice {
                variant_id: medium.to_owned(),
                amount_cents: 2100,
            },
        ],
        "EUR",
    )
    .await
    .unwrap();
    let repriced = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(repriced.compare_at(Some(small)), None);
    assert_eq!(repriced.variant(small).unwrap().compare_at_cents, None);
    assert_eq!(repriced.compare_at_ceiling(Some(medium)), Some(2290));
}
