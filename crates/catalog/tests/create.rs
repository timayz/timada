mod common;

use common::*;
use timada_catalog::{CatalogError, CreateProduct, MAX_SLUG_LEN, ProductStatus, SLUG_SUFFIX_LEN};

#[tokio::test]
async fn creating_a_product_makes_a_draft_with_a_derived_slug() {
    let cmd = command().await;
    let product = create_product(&cmd, "  Blue Mug  ").await;
    assert_eq!(product.slug, "blue-mug");
    assert_eq!(product.name, "Blue Mug");
    assert_eq!(product.description, "");
    assert_eq!(product.price, None);
    assert_eq!(product.status, ProductStatus::Draft);
    assert_eq!(product.created_by, ALICE);
    assert_eq!(product.created_at, product.updated_at);

    let by_slug = cmd.load_product_by_slug("blue-mug").await.unwrap().unwrap();
    assert_eq!(by_slug, product);
    assert_eq!(cmd.load_product(&product.id).await.unwrap(), Some(product));
}

#[tokio::test]
async fn a_taken_derived_slug_gets_a_short_suffix() {
    let cmd = command().await;
    let first = create_product(&cmd, "Mug").await;
    assert_eq!(first.slug, "mug");
    let second = create_product(&cmd, "MUG!").await;
    assert!(second.slug.starts_with("mug-"), "{}", second.slug);
    assert_eq!(second.slug.len(), "mug-".len() + SLUG_SUFFIX_LEN);
    assert_eq!(
        cmd.load_product_by_slug("mug").await.unwrap().unwrap().id,
        first.id,
        "the first product keeps the plain slug"
    );

    let long = "b".repeat(MAX_SLUG_LEN);
    let a = create_product(&cmd, &long).await;
    let b = create_product(&cmd, &long).await;
    assert_eq!(a.slug.len(), MAX_SLUG_LEN);
    assert_eq!(b.slug.len(), MAX_SLUG_LEN, "{}", b.slug);
    assert_ne!(a.slug, b.slug);
}

#[tokio::test]
async fn a_given_slug_is_kept_or_refused() {
    let cmd = command().await;
    let given = cmd
        .create_product(
            ALICE,
            CreateProduct {
                name: "Blue Mug".into(),
                slug: Some(" Mug-Blue ".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(given.slug, "mug-blue");

    let taken = cmd
        .create_product(
            ALICE,
            CreateProduct {
                name: "Another".into(),
                slug: Some("mug-blue".into()),
            },
        )
        .await;
    assert!(matches!(taken, Err(CatalogError::SlugTaken)), "{taken:?}");

    let invalid = cmd
        .create_product(
            ALICE,
            CreateProduct {
                name: "Another".into(),
                slug: Some("-bad-".into()),
            },
        )
        .await;
    assert!(
        matches!(invalid, Err(CatalogError::InvalidSlug)),
        "{invalid:?}"
    );

    let blank = cmd
        .create_product(
            ALICE,
            CreateProduct {
                name: "Another".into(),
                slug: Some("   ".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(blank.slug, "another", "a blank slug means: derive it");
}

#[tokio::test]
async fn names_are_validated() {
    let cmd = command().await;
    for bad in ["", "   ", &"n".repeat(121)] {
        let err = cmd.create_product(ALICE, create(bad)).await.unwrap_err();
        assert!(matches!(err, CatalogError::InvalidName), "{bad:?}: {err}");
        assert!(err.is_user_facing());
    }
    assert!(cmd.load_product_by_slug("product").await.unwrap().is_none());
}
