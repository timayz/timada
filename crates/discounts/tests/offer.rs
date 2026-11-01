mod common;

use common::*;
use timada_catalog::{CreateProduct, DefineVariant, OptionSelection};
use timada_discounts::{CreateOffer, DiscountError, MAX_OFFERS};

type Catalog = timada_catalog::Command<evento::Turso>;

/// A product at `amount_cents` EUR, published.
async fn product(catalog: &Catalog, name: &str, amount_cents: u64) -> String {
    let product = catalog
        .create_product(
            ALICE,
            CreateProduct {
                name: name.to_owned(),
                slug: None,
            },
        )
        .await
        .unwrap();
    catalog
        .price_product(ALICE, &product.id, amount_cents, "EUR")
        .await
        .unwrap();
    catalog.publish_product(ALICE, &product.id).await.unwrap();
    product.id
}

fn offer(product_id: &str, price_cents: u64) -> CreateOffer {
    CreateOffer {
        product_id: product_id.to_owned(),
        price_cents,
        currency: "eur".to_owned(),
        ..Default::default()
    }
}

#[tokio::test]
async fn an_offer_is_created_deactivated_and_archived() {
    let cmd = command().await;
    let catalog = timada_catalog::Command::new(cmd.executor().clone(), TENANT);
    let tracker = product(&catalog, "Tracker", 3000).await;
    let bowl = product(&catalog, "Bowl", 2000).await;
    assert!(cmd.offers().await.unwrap().is_empty());

    let id = cmd
        .create_offer(
            ALICE,
            CreateOffer {
                title: Some("  A tracker at 60 % off  ".into()),
                requires_product_id: Some(bowl.clone()),
                min_subtotal_cents: Some(1500),
                ..offer(&tracker, 1200)
            },
        )
        .await
        .unwrap();
    let offers = cmd.offers().await.unwrap();
    let created = &offers[0];
    assert_eq!(created.id, id);
    assert_eq!(created.currency, "EUR");
    assert_eq!(created.title.as_deref(), Some("A tracker at 60 % off"));
    assert_eq!(created.requires_product_id.as_deref(), Some(bowl.as_str()));
    assert!(created.active);
    assert_eq!(created.created_by, ALICE);

    cmd.deactivate_offer(ALICE, &id).await.unwrap();
    cmd.deactivate_offer(ALICE, &id).await.unwrap();
    assert!(!cmd.offers().await.unwrap()[0].active);
    cmd.reactivate_offer(ALICE, &id).await.unwrap();
    assert!(cmd.offers().await.unwrap()[0].active);
    cmd.archive_offer(ALICE, &id).await.unwrap();
    assert!(cmd.offers().await.unwrap().is_empty());
    assert!(matches!(
        cmd.archive_offer(ALICE, &id).await,
        Err(DiscountError::OfferNotFound)
    ));
}

#[tokio::test]
async fn what_is_refused_at_creation() {
    let cmd = command().await;
    let catalog = timada_catalog::Command::new(cmd.executor().clone(), TENANT);
    let tracker = product(&catalog, "Tracker", 3000).await;
    let shirt = catalog
        .create_product(
            ALICE,
            CreateProduct {
                name: "Shirt".into(),
                slug: None,
            },
        )
        .await
        .unwrap()
        .id;
    let size = catalog
        .define_option(ALICE, &shirt, "Size", vec!["S".into(), "M".into()])
        .await
        .unwrap();
    let small = catalog
        .define_variant(
            ALICE,
            &shirt,
            DefineVariant {
                selections: vec![OptionSelection {
                    option_id: size,
                    value: "S".into(),
                }],
                sku: None,
                amount_cents: 2500,
                currency: "EUR".into(),
                image_id: None,
            },
        )
        .await
        .unwrap();

    let cases = [
        (offer("nope", 1000), "offer_product_not_found"),
        (offer(&tracker, 0), "invalid_offer_price"),
        (offer(&tracker, 3000), "invalid_offer_price"),
        (
            CreateOffer {
                currency: "USD".into(),
                ..offer(&tracker, 1000)
            },
            "offer_currency",
        ),
        (offer(&shirt, 1000), "offer_variant"),
        (
            CreateOffer {
                variant_id: Some("other".into()),
                ..offer(&shirt, 1000)
            },
            "offer_variant",
        ),
        (
            CreateOffer {
                variant_id: Some(small.clone()),
                ..offer(&tracker, 1000)
            },
            "offer_variant",
        ),
        (
            CreateOffer {
                title: Some("x".repeat(81)),
                ..offer(&tracker, 1000)
            },
            "invalid_offer_title",
        ),
        (
            CreateOffer {
                requires_product_id: Some("nope".into()),
                ..offer(&tracker, 1000)
            },
            "offer_product_not_found",
        ),
        (
            CreateOffer {
                min_subtotal_cents: Some(0),
                ..offer(&tracker, 1000)
            },
            "invalid_min_subtotal",
        ),
        (
            CreateOffer {
                starts_at: Some(200),
                ends_at: Some(200),
                ..offer(&tracker, 1000)
            },
            "invalid_window",
        ),
    ];
    for (create, code) in cases {
        let refused = cmd.create_offer(ALICE, create.clone()).await.unwrap_err();
        assert_eq!(refused.code(), Some(code), "{create:?}");
    }

    // A variant below its own price is fine.
    cmd.create_offer(
        ALICE,
        CreateOffer {
            variant_id: Some(small),
            ..offer(&shirt, 2000)
        },
    )
    .await
    .unwrap();
    for _ in 1..MAX_OFFERS {
        cmd.create_offer(ALICE, offer(&tracker, 1000))
            .await
            .unwrap();
    }
    assert!(matches!(
        cmd.create_offer(ALICE, offer(&tracker, 1000)).await,
        Err(DiscountError::TooManyOffers)
    ));
}
