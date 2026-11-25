mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_catalog::{
    CatalogError, Classify, DefineVariant, FEED_LABELS, Identify, MAX_BRAND_LEN,
    MAX_FEED_LABEL_LEN, MAX_GOOGLE_CATEGORY_LEN, MAX_MPN_LEN, OptionSelection,
};

/// A product varying by colour and size, with the options' ids and the id
/// of its one variant.
async fn harness(cmd: &timada_catalog::Command<evento::Turso>) -> (String, String, String, String) {
    let product = create_product(cmd, "Cat Harness").await;
    let colour = cmd
        .define_option(ALICE, &product.id, "Colour", vec!["Beige".into()])
        .await
        .unwrap();
    let size = cmd
        .define_option(ALICE, &product.id, "Size", vec!["S".into()])
        .await
        .unwrap();
    let pick = |option_id: &str, value: &str| OptionSelection {
        option_id: option_id.to_owned(),
        value: value.to_owned(),
    };
    let variant = cmd
        .define_variant(
            ALICE,
            &product.id,
            DefineVariant {
                selections: vec![pick(&colour, "Beige"), pick(&size, "S")],
                sku: None,
                amount_cents: 1990,
                currency: "EUR".into(),
                image_id: None,
            },
        )
        .await
        .unwrap();
    (product.id, colour, size, variant)
}

fn identify(brand: &str, gtin: &str, mpn: &str) -> Identify {
    Identify {
        brand: Some(brand.to_owned()),
        gtin: Some(gtin.to_owned()),
        mpn: Some(mpn.to_owned()),
        no_identifier: false,
    }
}

#[tokio::test]
async fn a_product_says_who_makes_it_and_under_which_numbers() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    let id = mug.id.as_str();

    cmd.identify_product(ALICE, id, identify(" Acme ", "4006381 33393-1", " MUG-01 "))
        .await
        .unwrap();
    let known = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(known.brand.as_deref(), Some("Acme"));
    assert_eq!(
        known.gtin.as_deref(),
        Some("4006381333931"),
        "spaces and dashes are how it is printed, not part of it"
    );
    assert_eq!(known.mpn.as_deref(), Some("MUG-01"));
    assert!(!known.no_identifier);
    let version = known.aggregate_version().unwrap();

    // The same again writes nothing.
    cmd.identify_product(ALICE, id, identify("Acme", "4006381333931", "MUG-01"))
        .await
        .unwrap();
    let again = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version().unwrap(), version);

    // Empty fields clear them.
    cmd.identify_product(ALICE, id, identify(" ", "", ""))
        .await
        .unwrap();
    let cleared = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(
        (cleared.brand, cleared.gtin, cleared.mpn),
        (None, None, None)
    );
}

#[tokio::test]
async fn a_gtin_is_checked_by_its_last_digit() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    let with = |gtin: &str| Identify {
        gtin: Some(gtin.to_owned()),
        ..Default::default()
    };
    // The four lengths a barcode comes in.
    for gtin in [
        "73513537",
        "036000291452",
        "4006381333931",
        "10614141000415",
    ] {
        cmd.identify_product(ALICE, &mug.id, with(gtin))
            .await
            .unwrap_or_else(|err| panic!("{gtin}: {err}"));
    }
    for gtin in [
        "4006381333932",
        "400638133393",
        "40063813339310",
        "4006381E33931",
        "1234567",
    ] {
        let refused = cmd.identify_product(ALICE, &mug.id, with(gtin)).await;
        assert!(
            matches!(refused, Err(CatalogError::InvalidGtin)),
            "{gtin}: {refused:?}"
        );
    }
    let kept = cmd.load_product(&mug.id).await.unwrap().unwrap();
    assert_eq!(kept.gtin.as_deref(), Some("10614141000415"));
}

#[tokio::test]
async fn a_brand_and_a_part_number_have_a_length() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    let long_brand = cmd
        .identify_product(
            ALICE,
            &mug.id,
            Identify {
                brand: Some("é".repeat(MAX_BRAND_LEN + 1)),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(long_brand, Err(CatalogError::InvalidBrand)));
    let long_mpn = cmd
        .identify_product(
            ALICE,
            &mug.id,
            Identify {
                mpn: Some("x".repeat(MAX_MPN_LEN + 1)),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(long_mpn, Err(CatalogError::InvalidMpn)));
    cmd.identify_product(
        ALICE,
        &mug.id,
        Identify {
            brand: Some("é".repeat(MAX_BRAND_LEN)),
            mpn: Some("x".repeat(MAX_MPN_LEN)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_product_without_identifier_carries_no_number() {
    let cmd = command().await;
    let (product_id, _, _, variant) = harness(&cmd).await;
    let none = |gtin: Option<&str>| Identify {
        brand: Some("Acme".into()),
        gtin: gtin.map(str::to_owned),
        mpn: None,
        no_identifier: true,
    };

    let both = cmd
        .identify_product(ALICE, &product_id, none(Some("4006381333931")))
        .await;
    assert!(matches!(both, Err(CatalogError::IdentifierConflict)));

    // A variant's number counts too.
    cmd.identify_variant(ALICE, &product_id, &variant, None, Some("H-BEIGE-S"))
        .await
        .unwrap();
    let numbered = cmd.identify_product(ALICE, &product_id, none(None)).await;
    assert!(matches!(numbered, Err(CatalogError::IdentifierConflict)));

    cmd.identify_variant(ALICE, &product_id, &variant, None, None)
        .await
        .unwrap();
    cmd.identify_product(ALICE, &product_id, none(None))
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert!(state.no_identifier);
    assert_eq!(state.brand.as_deref(), Some("Acme"), "a brand is no number");

    // And none comes back while it says so.
    let late = cmd
        .identify_variant(ALICE, &product_id, &variant, Some("4006381333931"), None)
        .await;
    assert!(matches!(late, Err(CatalogError::IdentifierConflict)));
}

#[tokio::test]
async fn a_variant_has_numbers_of_its_own() {
    let cmd = command().await;
    let (product_id, _, _, variant) = harness(&cmd).await;

    cmd.identify_variant(
        ALICE,
        &product_id,
        &variant,
        Some("0360-0029-1452"),
        Some(" H-BEIGE-S "),
    )
    .await
    .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    let known = state.variant(&variant).unwrap();
    assert_eq!(known.gtin.as_deref(), Some("036000291452"));
    assert_eq!(known.mpn.as_deref(), Some("H-BEIGE-S"));
    assert_eq!(state.gtin, None, "the product's own number is another");
    let version = state.aggregate_version().unwrap();

    cmd.identify_variant(
        ALICE,
        &product_id,
        &variant,
        Some("036000291452"),
        Some("H-BEIGE-S"),
    )
    .await
    .unwrap();
    let again = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version().unwrap(), version);

    let bad = cmd
        .identify_variant(ALICE, &product_id, &variant, Some("123"), None)
        .await;
    assert!(matches!(bad, Err(CatalogError::InvalidGtin)));
    let unknown = cmd
        .identify_variant(ALICE, &product_id, "nope", None, None)
        .await;
    assert!(matches!(unknown, Err(CatalogError::VariantNotFound)));
}

#[tokio::test]
async fn a_product_is_filed_for_a_feed() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    let id = mug.id.as_str();

    cmd.classify_product(
        ALICE,
        id,
        Classify {
            google_category: Some(" Home & Garden > Kitchen & Dining > Tableware ".into()),
            age_group: Some("Adult".into()),
            gender: Some(" unisex ".into()),
        },
    )
    .await
    .unwrap();
    let filed = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(
        filed.google_category.as_deref(),
        Some("Home & Garden > Kitchen & Dining > Tableware")
    );
    assert_eq!(filed.age_group.as_deref(), Some("adult"));
    assert_eq!(filed.gender.as_deref(), Some("unisex"));
    let version = filed.aggregate_version().unwrap();

    cmd.classify_product(
        ALICE,
        id,
        Classify {
            google_category: Some("Home & Garden > Kitchen & Dining > Tableware".into()),
            age_group: Some("adult".into()),
            gender: Some("unisex".into()),
        },
    )
    .await
    .unwrap();
    let again = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version().unwrap(), version);

    for cmd_in in [
        Classify {
            age_group: Some("teen".into()),
            ..Default::default()
        },
        Classify {
            gender: Some("cats".into()),
            ..Default::default()
        },
    ] {
        let refused = cmd.classify_product(ALICE, id, cmd_in).await;
        assert!(matches!(refused, Err(CatalogError::InvalidAudience)));
    }
    let long = cmd
        .classify_product(
            ALICE,
            id,
            Classify {
                google_category: Some("x".repeat(MAX_GOOGLE_CATEGORY_LEN + 1)),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(long, Err(CatalogError::InvalidGoogleCategory)));

    // Its number is a category too, and everything empty clears.
    cmd.classify_product(
        ALICE,
        id,
        Classify {
            google_category: Some("2271".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let numbered = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(numbered.google_category.as_deref(), Some("2271"));
    assert_eq!((numbered.age_group, numbered.gender), (None, None));
}

#[tokio::test]
async fn an_option_is_of_one_kind_and_a_kind_has_one_option() {
    let cmd = command().await;
    let (product_id, colour, size, _) = harness(&cmd).await;

    cmd.set_option_kind(ALICE, &product_id, &colour, Some("Color"))
        .await
        .unwrap();
    let taken = cmd
        .set_option_kind(ALICE, &product_id, &size, Some("color"))
        .await;
    assert!(matches!(taken, Err(CatalogError::OptionKindTaken)));
    let unknown = cmd
        .set_option_kind(ALICE, &product_id, &size, Some("flavour"))
        .await;
    assert!(matches!(unknown, Err(CatalogError::InvalidOptionKind)));
    let missing = cmd
        .set_option_kind(ALICE, &product_id, "nope", Some("size"))
        .await;
    assert!(matches!(missing, Err(CatalogError::OptionNotFound)));
    cmd.set_option_kind(ALICE, &product_id, &size, Some("size"))
        .await
        .unwrap();

    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(
        state.option(&colour).unwrap().kind.as_deref(),
        Some("color")
    );
    assert_eq!(state.option(&size).unwrap().kind.as_deref(), Some("size"));
    let version = state.aggregate_version().unwrap();
    cmd.set_option_kind(ALICE, &product_id, &size, Some("size"))
        .await
        .unwrap();

    // Cleared, the kind is free for the other option.
    cmd.set_option_kind(ALICE, &product_id, &colour, None)
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.aggregate_version().unwrap(), version + 1);
    assert_eq!(state.option(&colour).unwrap().kind, None);
}

#[tokio::test]
async fn five_labels_set_together_or_one_at_a_time() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    let id = mug.id.as_str();

    cmd.set_feed_labels(
        ALICE,
        id,
        &[" high margin ".into(), String::new(), "eu".into()],
    )
    .await
    .unwrap();
    let labelled = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(labelled.feed_labels, ["high margin", "", "eu", "", ""]);
    let version = labelled.aggregate_version().unwrap();

    // One label moves, the four others stay.
    cmd.set_feed_label(ALICE, id, 4, "winter").await.unwrap();
    cmd.set_feed_label(ALICE, id, 0, "high margin")
        .await
        .unwrap();
    let one = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(one.feed_labels, ["high margin", "", "eu", "", "winter"]);
    assert_eq!(one.aggregate_version().unwrap(), version + 1);
    cmd.set_feed_label(ALICE, id, 2, " ").await.unwrap();
    let cleared = cmd.load_product(id).await.unwrap().unwrap();
    assert_eq!(cleared.feed_labels, ["high margin", "", "", "", "winter"]);

    let sixth = cmd.set_feed_label(ALICE, id, FEED_LABELS, "x").await;
    assert!(matches!(sixth, Err(CatalogError::InvalidFeedLabel)));
    let six = cmd
        .set_feed_labels(ALICE, id, &vec![String::new(); 6])
        .await;
    assert!(matches!(six, Err(CatalogError::InvalidFeedLabel)));
    let long = cmd
        .set_feed_label(ALICE, id, 1, &"é".repeat(MAX_FEED_LABEL_LEN + 1))
        .await;
    assert!(matches!(long, Err(CatalogError::InvalidFeedLabel)));
    cmd.set_feed_label(ALICE, id, 1, &"é".repeat(MAX_FEED_LABEL_LEN))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_product_says_what_a_machine_wrote() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    let id = mug.id.as_str();

    cmd.declare_ai_writing(ALICE, id, false, true)
        .await
        .unwrap();
    let declared = cmd.load_product(id).await.unwrap().unwrap();
    assert!(!declared.ai_title && declared.ai_description);
    let version = declared.aggregate_version().unwrap();
    cmd.declare_ai_writing(ALICE, id, false, true)
        .await
        .unwrap();
    cmd.declare_ai_writing(ALICE, id, false, false)
        .await
        .unwrap();
    let human = cmd.load_product(id).await.unwrap().unwrap();
    assert!(!human.ai_title && !human.ai_description);
    assert_eq!(human.aggregate_version().unwrap(), version + 1);
}

#[tokio::test]
async fn an_archived_product_takes_none_of_it() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    cmd.archive_product(ALICE, &mug.id).await.unwrap();
    let id = mug.id.as_str();
    assert!(matches!(
        cmd.identify_product(ALICE, id, Identify::default()).await,
        Err(CatalogError::Archived)
    ));
    assert!(matches!(
        cmd.classify_product(ALICE, id, Classify::default()).await,
        Err(CatalogError::Archived)
    ));
    assert!(matches!(
        cmd.set_feed_label(ALICE, id, 0, "x").await,
        Err(CatalogError::Archived)
    ));
    assert!(matches!(
        cmd.declare_ai_writing(ALICE, id, true, true).await,
        Err(CatalogError::Archived)
    ));
}

#[tokio::test]
async fn a_product_leaves_the_feed_and_comes_back() {
    let cmd = command().await;
    let mug = create_product(&cmd, "Blue Mug").await;
    let id = mug.id.as_str();
    // A draft is in no feed, whatever it says.
    assert!(!cmd.load_product(id).await.unwrap().unwrap().is_in_feed());
    cmd.price_product(ALICE, id, 1250, "EUR").await.unwrap();
    cmd.publish_product(ALICE, id).await.unwrap();
    let listed = cmd.load_product(id).await.unwrap().unwrap();
    assert!(listed.is_in_feed(), "a published product is in the feed");
    let version = listed.aggregate_version().unwrap();

    cmd.list_in_feed(ALICE, id, true).await.unwrap();
    cmd.list_in_feed(ALICE, id, false).await.unwrap();
    cmd.list_in_feed(ALICE, id, false).await.unwrap();
    let out = cmd.load_product(id).await.unwrap().unwrap();
    assert!(out.off_feed && !out.is_in_feed());
    assert!(out.is_published(), "it stays in the shop");
    assert_eq!(out.aggregate_version().unwrap(), version + 1);

    cmd.list_in_feed(ALICE, id, true).await.unwrap();
    assert!(cmd.load_product(id).await.unwrap().unwrap().is_in_feed());
}

#[tokio::test]
async fn a_feed_opens_at_an_address_of_its_own_and_moves_or_closes() {
    let cmd = command().await;
    assert_eq!(cmd.load_feed_token().await.unwrap(), None);
    // Closing what is not open writes nothing.
    cmd.close_feed(ALICE).await.unwrap();

    let first = cmd.open_feed(ALICE).await.unwrap();
    assert_eq!(first.len(), 64);
    assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(cmd.load_feed_token().await.unwrap(), Some(first.clone()));

    // Opened again, it moves: the address it had is gone.
    let second = cmd.open_feed(ALICE).await.unwrap();
    assert_ne!(first, second);
    assert_eq!(cmd.load_feed_token().await.unwrap(), Some(second));

    cmd.close_feed(ALICE).await.unwrap();
    assert_eq!(cmd.load_feed_token().await.unwrap(), None);
    let third = cmd.open_feed(ALICE).await.unwrap();
    assert_eq!(cmd.load_feed_token().await.unwrap(), Some(third));
}

#[tokio::test]
async fn a_product_is_kept_out_of_ads_and_let_back_in() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cat Tree").await;
    assert!(!product.out_of_ads);
    cmd.set_product_in_ads(ALICE, &product.id, false)
        .await
        .unwrap();
    let kept = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert!(kept.out_of_ads);
    // It stays in the shop as it was.
    assert_eq!(kept.status, product.status);
    // The same again writes nothing.
    cmd.set_product_in_ads(ALICE, &product.id, false)
        .await
        .unwrap();
    let again = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(again.aggregate_version, kept.aggregate_version);
    cmd.set_product_in_ads(ALICE, &product.id, true)
        .await
        .unwrap();
    let back = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert!(!back.out_of_ads);
}
