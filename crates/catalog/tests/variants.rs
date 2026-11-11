mod common;

use common::*;
use timada_catalog::{
    AddImage, CatalogError, DefineVariant, MAX_OPTION_VALUES, MAX_OPTIONS, OptionLabels,
    OptionSelection, StockSummary,
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
async fn with_colours(cmd: &timada_catalog::Command<evento::Turso>) -> (String, String) {
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
    assert_eq!(state.variants[0].price.to_string(), "22.90 €");
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
        "24.90 €"
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
async fn an_option_and_its_values_show_labels_without_moving() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    let beige = cmd
        .define_variant(ALICE, &product_id, sell(vec![pick(&colour, "Beige")], 2290))
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.options[0].display_name(), "Colour");
    assert_eq!(state.options[0].display_value("Beige"), "Beige");
    assert_eq!(
        state.variant_label(&state.variants[0]).as_deref(),
        Some("Beige")
    );

    // Labels are the one thing about an option that changes once variants
    // are sold; they are trimmed, and the name and values do not move.
    cmd.label_option(
        ALICE,
        &product_id,
        &colour,
        OptionLabels {
            label: Some(" Couleur ".into()),
            values: vec![("Beige".into(), Some(" Sable ".into()))],
        },
    )
    .await
    .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    let option = &state.options[0];
    assert_eq!(option.name, "Colour");
    assert_eq!(option.values, ["Beige", "Blue"]);
    assert_eq!(option.label.as_deref(), Some("Couleur"));
    assert_eq!(option.display_name(), "Couleur");
    assert_eq!(option.display_value("Beige"), "Sable");
    assert_eq!(option.display_value("Blue"), "Blue");
    assert_eq!(
        state.variant_label(&state.variants[0]).as_deref(),
        Some("Sable")
    );
    assert_eq!(
        state
            .variant_for(&[pick(&colour, "Beige")])
            .map(|v| v.id.as_str()),
        Some(beige.as_str()),
        "the combination is still found by its value"
    );

    // Labels already as asked write nothing; a value left out keeps its.
    let before = state.updated_at;
    cmd.label_option(
        ALICE,
        &product_id,
        &colour,
        OptionLabels {
            label: Some("Couleur".into()),
            values: vec![],
        },
    )
    .await
    .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.updated_at, before);
    assert_eq!(state.options[0].display_value("Beige"), "Sable");

    // Too long, an unknown value, an unknown option: refused.
    let too_long = cmd
        .label_option(
            ALICE,
            &product_id,
            &colour,
            OptionLabels {
                label: Some("x".repeat(41)),
                values: vec![],
            },
        )
        .await;
    assert!(
        matches!(too_long, Err(CatalogError::InvalidOptionLabel)),
        "{too_long:?}"
    );
    let unknown_value = cmd
        .label_option(
            ALICE,
            &product_id,
            &colour,
            OptionLabels {
                label: None,
                values: vec![("Pink".into(), Some("Rose".into()))],
            },
        )
        .await;
    assert!(
        matches!(unknown_value, Err(CatalogError::OptionValueNotFound)),
        "{unknown_value:?}"
    );
    let unknown_option = cmd
        .label_option(ALICE, &product_id, "nope", OptionLabels::default())
        .await;
    assert!(
        matches!(unknown_option, Err(CatalogError::OptionNotFound)),
        "{unknown_option:?}"
    );

    // Empty labels go back to the name and the value.
    cmd.label_option(
        ALICE,
        &product_id,
        &colour,
        OptionLabels {
            label: Some("  ".into()),
            values: vec![("Beige".into(), None)],
        },
    )
    .await
    .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.options[0].label, None);
    assert!(state.options[0].value_labels.is_empty());
    assert_eq!(
        state.variant_label(&state.variants[0]).as_deref(),
        Some("Beige")
    );
}

/// A product shipping from China in two colours and from France in one: the
/// product and both options.
async fn with_warehouses(cmd: &timada_catalog::Command<evento::Turso>) -> (String, String, String) {
    let product = create_product(cmd, "Cat Tunnel").await;
    let from = cmd
        .define_option(
            ALICE,
            &product.id,
            "Ships From",
            vec!["China".into(), "France".into()],
        )
        .await
        .unwrap();
    let colour = cmd
        .define_option(
            ALICE,
            &product.id,
            "Colour",
            vec!["Rose".into(), "Blue".into()],
        )
        .await
        .unwrap();
    for (warehouse, shade, amount_cents) in [
        ("China", "Rose", 1000),
        ("China", "Blue", 1100),
        ("France", "Rose", 1500),
    ] {
        cmd.define_variant(
            ALICE,
            &product.id,
            sell(
                vec![pick(&from, warehouse), pick(&colour, shade)],
                amount_cents,
            ),
        )
        .await
        .unwrap();
    }
    (product.id, from, colour)
}

#[tokio::test]
async fn a_hidden_option_leaves_one_of_its_values_on_sale() {
    let cmd = command().await;
    let (product_id, from, colour) = with_warehouses(&cmd).await;
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.sold_variants().count(), 3);
    assert_eq!(state.price_range(), Some((1000, 1500, "EUR")));
    assert_eq!(
        state.variant_label(&state.variants[0]).as_deref(),
        Some("China / Rose")
    );

    cmd.hide_option(ALICE, &product_id, &from, " France ")
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(
        state.option(&from).unwrap().fixed_value.as_deref(),
        Some("France")
    );

    // A shopper finds the French combination alone, and reads nothing of
    // where it ships from.
    let sold: Vec<_> = state.sold_variants().collect();
    assert_eq!(sold.len(), 1);
    assert_eq!(sold[0].value_of(&from), Some("France"));
    assert_eq!(state.price_range(), Some((1500, 1500, "EUR")));
    assert_eq!(state.variant_label(sold[0]).as_deref(), Some("Rose"));
    assert_eq!(
        state
            .shown_options()
            .map(|option| option.name.as_str())
            .collect::<Vec<_>>(),
        ["Colour"]
    );
    assert_eq!(
        state
            .values_on_sale(state.option(&colour).unwrap())
            .collect::<Vec<_>>(),
        ["Rose"],
        "no blue ships from France"
    );

    // The others stay, off sale: an import still finds them, by every value.
    assert_eq!(state.variants.len(), 3);
    let china = state
        .variant_for(&[pick(&from, "China"), pick(&colour, "Rose")])
        .unwrap();
    assert!(!state.sells(china));
    assert!(state.variant(&china.id).is_some());
    assert!(state.sold_variant(&china.id).is_none());
    assert!(state.sold_variant(&sold[0].id).is_some());

    // The same value again writes nothing; another moves what is on sale.
    let version = state.aggregate_version;
    cmd.hide_option(ALICE, &product_id, &from, "France")
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.aggregate_version, version);
    cmd.hide_option(ALICE, &product_id, &from, "China")
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.price_range(), Some((1000, 1100, "EUR")));
    assert_eq!(
        state
            .values_on_sale(state.option(&colour).unwrap())
            .collect::<Vec<_>>(),
        ["Rose", "Blue"]
    );

    // Shown again, every combination is back; twice writes nothing.
    cmd.show_option(ALICE, &product_id, &from).await.unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.option(&from).unwrap().fixed_value, None);
    assert_eq!(state.sold_variants().count(), 3);
    let version = state.aggregate_version;
    cmd.show_option(ALICE, &product_id, &from).await.unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.aggregate_version, version);
}

#[tokio::test]
async fn an_option_is_hidden_only_where_something_stays_on_sale() {
    let cmd = command().await;
    let (product_id, from, colour) = with_warehouses(&cmd).await;
    cmd.change_option_values(
        ALICE,
        &product_id,
        &from,
        vec!["China".into(), "France".into(), "Spain".into()],
    )
    .await
    .unwrap();

    let unknown = cmd.hide_option(ALICE, &product_id, "nope", "France").await;
    assert!(
        matches!(unknown, Err(CatalogError::OptionNotFound)),
        "{unknown:?}"
    );
    let not_a_value = cmd.hide_option(ALICE, &product_id, &from, "Italy").await;
    assert!(
        matches!(not_a_value, Err(CatalogError::OptionValueNotFound)),
        "{not_a_value:?}"
    );
    let no_variant = cmd.hide_option(ALICE, &product_id, &from, "Spain").await;
    assert!(
        matches!(no_variant, Err(CatalogError::OptionValueNotSold)),
        "{no_variant:?}"
    );

    // Blue is sold, but not from France: the two together sell nothing.
    cmd.hide_option(ALICE, &product_id, &from, "France")
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert!(state.can_hide_at(&colour, "Rose"));
    assert!(!state.can_hide_at(&colour, "Blue"));
    assert!(state.can_hide_at(&from, "China"), "its own value moves");
    let nothing_left = cmd.hide_option(ALICE, &product_id, &colour, "Blue").await;
    assert!(
        matches!(nothing_left, Err(CatalogError::OptionValueNotSold)),
        "{nothing_left:?}"
    );

    cmd.archive_product(ALICE, &product_id).await.unwrap();
    let archived = cmd.show_option(ALICE, &product_id, &from).await;
    assert!(
        matches!(archived, Err(CatalogError::Archived)),
        "{archived:?}"
    );
}

#[tokio::test]
async fn every_option_hidden_leaves_one_variant_with_nothing_to_read() {
    let cmd = command().await;
    let (product_id, from, colour) = with_warehouses(&cmd).await;
    cmd.hide_option(ALICE, &product_id, &from, "China")
        .await
        .unwrap();
    cmd.hide_option(ALICE, &product_id, &colour, "Blue")
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.shown_options().count(), 0);
    let sold: Vec<_> = state.sold_variants().collect();
    assert_eq!(sold.len(), 1);
    assert_eq!(state.price_range(), Some((1100, 1100, "EUR")));
    assert_eq!(state.shown_selections(sold[0]).count(), 0);
    assert_eq!(state.variant_label(sold[0]), None);
}

#[tokio::test]
async fn nothing_on_sale_cannot_be_published() {
    let cmd = command().await;
    let (product_id, from, _) = with_warehouses(&cmd).await;
    cmd.hide_option(ALICE, &product_id, &from, "France")
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert!(state.is_priced());
    let french = state.sold_variants().next().unwrap().id.clone();

    // The only French combination goes, though others remain: the product
    // has a price for none a shopper can buy.
    cmd.remove_variant(ALICE, &product_id, &french)
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(state.variants.len(), 2);
    assert!(!state.is_priced());
    assert_eq!(state.price_range(), None);
    let refused = cmd.publish_product(ALICE, &product_id).await;
    assert!(
        matches!(refused, Err(CatalogError::NotPriced)),
        "{refused:?}"
    );

    // The value stays while the option is hidden at it.
    let dropped = cmd
        .change_option_values(ALICE, &product_id, &from, vec!["China".into()])
        .await;
    assert!(
        matches!(dropped, Err(CatalogError::OptionHiddenAtValue)),
        "{dropped:?}"
    );
    cmd.show_option(ALICE, &product_id, &from).await.unwrap();
    cmd.change_option_values(ALICE, &product_id, &from, vec!["China".into()])
        .await
        .unwrap();
    cmd.publish_product(ALICE, &product_id).await.unwrap();
}

#[tokio::test]
async fn what_is_off_sale_does_not_count_as_in_stock() {
    let cmd = command().await;
    let (product_id, from, _) = with_warehouses(&cmd).await;
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    let french = state
        .variants
        .iter()
        .find(|variant| variant.value_of(&from) == Some("France"))
        .unwrap()
        .id
        .clone();
    // Plenty in China, none left in France.
    let on_hand = |variant_id: Option<&str>| Some(if variant_id == Some(&french) { 0 } else { 5 });
    assert_eq!(StockSummary::of(&state, on_hand), StockSummary::InStock);

    cmd.hide_option(ALICE, &product_id, &from, "France")
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(StockSummary::of(&state, on_hand), StockSummary::SoldOut);
}

#[tokio::test]
async fn a_value_that_leaves_an_option_takes_its_label_with_it() {
    let cmd = command().await;
    let (product_id, colour) = with_colours(&cmd).await;
    cmd.label_option(
        ALICE,
        &product_id,
        &colour,
        OptionLabels {
            label: None,
            values: vec![
                ("Beige".into(), Some("Sable".into())),
                ("Blue".into(), Some("Bleu".into())),
            ],
        },
    )
    .await
    .unwrap();
    cmd.change_option_values(ALICE, &product_id, &colour, vec!["Blue".into()])
        .await
        .unwrap();
    let state = cmd.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(
        state.options[0].value_labels,
        [("Blue".to_owned(), "Bleu".to_owned())]
            .into_iter()
            .collect()
    );
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
    run_releases(&cmd).await;
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
    run_releases(&cmd).await;
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
            picture: test_picture(),
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
