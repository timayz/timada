mod common;

use common::*;
use timada_catalog::{
    AddImage, CatalogError, DefineVariant, MAX_OPTION_VALUES, MAX_OPTIONS, OptionSelection,
};

fn pick(option_id: &str, value: &str) -> OptionSelection {
    OptionSelection {
        option_id: option_id.to_owned(),
        value: value.to_owned(),
    }
}

fn sell(selections: Vec<OptionSelection>, amount_cents: u64) -> DefineVariant {
    DefineVariant {
        selections,
        sku: None,
        amount_cents,
        currency: "EUR".into(),
        image_id: None,
    }
}

/// A product varying by colour, with the option's id.
async fn with_colours(cmd: &timada_catalog::Command<evento::Sqlite>) -> (String, String) {
    let product = create_product(cmd, "Cat Harness").await;
    let colour = cmd
        .define_option(
            ALICE,
            &product.id,
            " Colour ",
            vec!["Beige".into(), "Blue".into(), " ".into(), "beige".into()],
        )
        .await
        .unwrap();
    (product.id, colour)
}

#[tokio::test]
async fn an_option_keeps_the_values_the_merchant_gave_it() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.options.len(), 1);
    assert_eq!(state.options[0].id, colour);
    assert_eq!(state.options[0].name, "Colour");
    assert_eq!(
        state.options[0].values,
        ["Beige", "Blue"],
        "blank values go, and a repeat in another case is one value"
    );

    cmd.rename_option(ALICE, &product_id, &colour, "Shade")
        .await
        .unwrap();
    cmd.change_option_values(
        ALICE,
        &product_id,
        &colour,
        vec!["Beige".into(), "Grey".into()],
    )
    .await
    .unwrap();
    let edited = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(edited.options[0].name, "Shade");
    assert_eq!(edited.options[0].values, ["Beige", "Grey"]);

    let version = edited.aggregate_version;
    cmd.rename_option(ALICE, &product_id, &colour, " Shade ")
        .await
        .unwrap();
    assert_eq!(
        cmd.load_product(&product_id)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        version,
        "the same name writes nothing"
    );

    let twice = cmd
        .define_option(ALICE, &product_id, "shade", vec!["Red".into()])
        .await;
    assert!(
        matches!(twice, Err(CatalogError::OptionExists)),
        "{twice:?}"
    );
    let empty = cmd
        .define_option(ALICE, &product_id, "Size", vec!["  ".into()])
        .await;
    assert!(
        matches!(empty, Err(CatalogError::InvalidOptionValues)),
        "{empty:?}"
    );
    let too_many = cmd
        .define_option(
            ALICE,
            &product_id,
            "Pattern",
            (0..=MAX_OPTION_VALUES).map(|n| n.to_string()).collect(),
        )
        .await;
    assert!(
        matches!(too_many, Err(CatalogError::InvalidOptionValues)),
        "{too_many:?}"
    );
}

#[tokio::test]
async fn a_product_varies_in_three_ways_at_most() {
    let cmd = command().await;
    let product = create_product(&cmd, "Cat Harness").await;
    for n in 0..MAX_OPTIONS {
        cmd.define_option(
            ALICE,
            &product.id,
            &format!("Option {n}"),
            vec!["One".into()],
        )
        .await
        .unwrap();
    }
    let err = cmd
        .define_option(ALICE, &product.id, "One too many", vec!["One".into()])
        .await;
    assert!(matches!(err, Err(CatalogError::TooManyOptions)), "{err:?}");
}

#[tokio::test]
async fn a_variant_names_every_option_once_and_prices_itself() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    let size = cmd
        .define_option(ALICE, &product_id, "Size", vec!["S".into(), "M".into()])
        .await
        .unwrap();

    let beige_s = cmd
        .define_variant(
            ALICE,
            &product_id,
            sell(vec![pick(&size, "S"), pick(&colour, "Beige")], 2290),
        )
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.variants.len(), 1);
    assert_eq!(
        state.variants[0].selections,
        vec![pick(&colour, "Beige"), pick(&size, "S")],
        "the product's own option order, not the caller's"
    );
    assert_eq!(state.variants[0].price.to_string(), "22.90 EUR");
    assert_eq!(
        state
            .variant_for(&[pick(&size, "S"), pick(&colour, "Beige")])
            .map(|v| v.id.as_str()),
        Some(beige_s.as_str()),
        "a lookup does not care about order either"
    );

    let same = cmd
        .define_variant(
            ALICE,
            &product_id,
            sell(vec![pick(&colour, "Beige"), pick(&size, "S")], 9900),
        )
        .await;
    assert!(matches!(same, Err(CatalogError::VariantExists)), "{same:?}");
    for bad in [
        vec![pick(&colour, "Beige")],
        vec![pick(&colour, "Beige"), pick(&colour, "Blue")],
        vec![pick(&colour, "Purple"), pick(&size, "S")],
        vec![pick(&colour, "Beige"), pick("no-such-option", "S")],
    ] {
        let err = cmd
            .define_variant(ALICE, &product_id, sell(bad.clone(), 1000))
            .await;
        assert!(
            matches!(err, Err(CatalogError::InvalidSelection)),
            "{bad:?}: {err:?}"
        );
    }

    cmd.price_variant(ALICE, &product_id, &beige_s, 2490, "EUR")
        .await
        .unwrap();
    assert_eq!(
        cmd.load_product(&product_id)
            .await
            .unwrap()
            .unwrap()
            .variants[0]
            .price
            .to_string(),
        "24.90 EUR"
    );
    let missing = cmd
        .price_variant(ALICE, &product_id, "nope", 1000, "EUR")
        .await;
    assert!(
        matches!(missing, Err(CatalogError::VariantNotFound)),
        "{missing:?}"
    );
}

#[tokio::test]
async fn every_price_of_a_product_is_in_one_currency() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    cmd.define_variant(ALICE, &product_id, sell(vec![pick(&colour, "Beige")], 2290))
        .await
        .unwrap();

    let other = cmd
        .define_variant(
            ALICE,
            &product_id,
            DefineVariant {
                currency: "USD".into(),
                ..sell(vec![pick(&colour, "Blue")], 2290)
            },
        )
        .await;
    assert!(
        matches!(other, Err(CatalogError::CurrencyMismatch)),
        "{other:?}"
    );
    let base = cmd.price_product(ALICE, &product_id, 1000, "GBP").await;
    assert!(
        matches!(base, Err(CatalogError::CurrencyMismatch)),
        "{base:?}"
    );
    cmd.price_product(ALICE, &product_id, 1000, "EUR")
        .await
        .expect("the same currency is fine");
}

#[tokio::test]
async fn a_price_range_covers_what_a_shopper_may_pay() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    assert_eq!(
        cmd.load_product(&product_id)
            .await
            .unwrap()
            .unwrap()
            .price_range(),
        None,
        "nothing priced, no range"
    );

    cmd.define_variant(ALICE, &product_id, sell(vec![pick(&colour, "Beige")], 2290))
        .await
        .unwrap();
    cmd.define_variant(ALICE, &product_id, sell(vec![pick(&colour, "Blue")], 1990))
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.price_range(), Some((1990, 2290, "EUR")));
    assert_eq!(state.currency(), Some("EUR"));
    assert!(state.is_priced());
}

#[tokio::test]
async fn a_product_with_variants_is_published_without_a_price_of_its_own() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    let unpriced = cmd.publish_product(ALICE, &product_id).await;
    assert!(
        matches!(unpriced, Err(CatalogError::NotPriced)),
        "{unpriced:?}"
    );

    cmd.define_variant(ALICE, &product_id, sell(vec![pick(&colour, "Beige")], 2290))
        .await
        .unwrap();
    cmd.publish_product(ALICE, &product_id)
        .await
        .expect("a variant to pick is price enough");
    assert!(
        cmd.load_product(&product_id)
            .await
            .unwrap()
            .unwrap()
            .is_published()
    );
}

#[tokio::test]
async fn the_options_settle_before_the_variants_do() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    cmd.define_variant(ALICE, &product_id, sell(vec![pick(&colour, "Beige")], 2290))
        .await
        .unwrap();

    for err in [
        cmd.define_option(ALICE, &product_id, "Size", vec!["S".into()])
            .await
            .map(|_| ()),
        cmd.remove_option(ALICE, &product_id, &colour).await,
    ] {
        assert!(matches!(err, Err(CatalogError::OptionInUse)), "{err:?}");
    }
    let dropped = cmd
        .change_option_values(ALICE, &product_id, &colour, vec!["Blue".into()])
        .await;
    assert!(
        matches!(dropped, Err(CatalogError::OptionValueInUse)),
        "a value a variant uses cannot go: {dropped:?}"
    );
    cmd.change_option_values(
        ALICE,
        &product_id,
        &colour,
        vec!["Beige".into(), "Grey".into()],
    )
    .await
    .expect("but the rest of the list can change");

    cmd.remove_variant(
        ALICE,
        &product_id,
        &cmd.load_product(&product_id)
            .await
            .unwrap()
            .unwrap()
            .variants[0]
            .id
            .clone(),
    )
    .await
    .unwrap();
    cmd.remove_option(ALICE, &product_id, &colour)
        .await
        .expect("and once nothing sells, the option can go");
    assert!(
        cmd.load_product(&product_id)
            .await
            .unwrap()
            .unwrap()
            .options
            .is_empty()
    );
}

#[tokio::test]
async fn a_stock_code_means_one_variant_across_the_tenant() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    let beige = cmd
        .define_variant(
            ALICE,
            &product_id,
            DefineVariant {
                sku: Some(" harness-beige ".into()),
                ..sell(vec![pick(&colour, "Beige")], 2290)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        cmd.load_product(&product_id)
            .await
            .unwrap()
            .unwrap()
            .variants[0]
            .sku
            .as_deref(),
        Some("harness-beige")
    );

    let taken = cmd
        .define_variant(
            ALICE,
            &product_id,
            DefineVariant {
                sku: Some("HARNESS-BEIGE".into()),
                ..sell(vec![pick(&colour, "Blue")], 2290)
            },
        )
        .await;
    assert!(
        matches!(taken, Err(CatalogError::SkuTaken)),
        "case does not make a second code: {taken:?}"
    );
    // And not on another product either.
    let (other_id, other_colour) = with_colours(&cmd).await;
    let elsewhere = cmd
        .define_variant(
            ALICE,
            &other_id,
            DefineVariant {
                sku: Some("harness-beige".into()),
                ..sell(vec![pick(&other_colour, "Beige")], 2290)
            },
        )
        .await;
    assert!(
        matches!(elsewhere, Err(CatalogError::SkuTaken)),
        "{elsewhere:?}"
    );
    let bad = cmd
        .define_variant(
            ALICE,
            &product_id,
            DefineVariant {
                sku: Some("not a code!".into()),
                ..sell(vec![pick(&colour, "Blue")], 2290)
            },
        )
        .await;
    assert!(matches!(bad, Err(CatalogError::InvalidSku)), "{bad:?}");

    // Changing it frees the old one for someone else.
    cmd.change_variant_sku(ALICE, &product_id, &beige, Some("harness-sand"))
        .await
        .unwrap();
    cmd.define_variant(
        ALICE,
        &other_id,
        DefineVariant {
            sku: Some("harness-beige".into()),
            ..sell(vec![pick(&other_colour, "Beige")], 2290)
        },
    )
    .await
    .expect("the code it let go is free");

    // Removing the variant frees its code too, and so does archiving.
    cmd.remove_variant(ALICE, &product_id, &beige)
        .await
        .unwrap();
    assert!(
        cmd.load_sku_index("harness-sand").await.unwrap().is_none(),
        "the code is free again"
    );
}

#[tokio::test]
async fn archiving_a_product_frees_the_codes_of_its_variants() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    cmd.define_variant(
        ALICE,
        &product_id,
        DefineVariant {
            sku: Some("gone-soon".into()),
            ..sell(vec![pick(&colour, "Beige")], 2290)
        },
    )
    .await
    .unwrap();
    cmd.archive_product(ALICE, &product_id).await.unwrap();
    assert!(cmd.load_sku_index("gone-soon").await.unwrap().is_none());

    let (other_id, other_colour) = with_colours(&cmd).await;
    cmd.define_variant(
        ALICE,
        &other_id,
        DefineVariant {
            sku: Some("gone-soon".into()),
            ..sell(vec![pick(&other_colour, "Beige")], 2290)
        },
    )
    .await
    .expect("another product may take it");
}

#[tokio::test]
async fn a_variant_shows_one_of_the_products_pictures() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    cmd.add_image(
        ALICE,
        &product_id,
        AddImage {
            image_id: "img-beige".into(),
            ext: "png".into(),
            size: 1024,
            width: None,
            height: None,
            alt: "The beige one".into(),
        },
    )
    .await
    .unwrap();

    let unknown = cmd
        .define_variant(
            ALICE,
            &product_id,
            DefineVariant {
                image_id: Some("img-nope".into()),
                ..sell(vec![pick(&colour, "Beige")], 2290)
            },
        )
        .await;
    assert!(
        matches!(unknown, Err(CatalogError::ImageNotFound)),
        "{unknown:?}"
    );

    let beige = cmd
        .define_variant(
            ALICE,
            &product_id,
            DefineVariant {
                image_id: Some("img-beige".into()),
                ..sell(vec![pick(&colour, "Beige")], 2290)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        cmd.load_product(&product_id)
            .await
            .unwrap()
            .unwrap()
            .variants[0]
            .image_id
            .as_deref(),
        Some("img-beige")
    );

    // A picture that goes takes its place on the variant with it.
    cmd.remove_image(ALICE, &product_id, "img-beige")
        .await
        .unwrap();
    let after = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(after.variant(&beige).unwrap().image_id, None);
}
