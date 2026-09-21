use evento::Executor;
use sqlx::SqlitePool;
use timada_catalog::{
    Brand, CatalogError, Command, CreateFamily, CreateProduct, FamilyOption, MAX_FAMILY_OPTIONS,
    OptionValue, family_by_id, family_id, family_list_subscription, family_of_product,
    family_variants, list_families, load_product_page, migrations, product_list_subscription,
    variant_choices,
};

async fn sync<E: Executor + Clone + 'static>(executor: &E, db: &SqlitePool) -> anyhow::Result<()> {
    family_list_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    // Strict, and fed the same products: it must let the family events by.
    product_list_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    Ok(())
}

fn product(sku: &str) -> CreateProduct {
    CreateProduct {
        sku: sku.into(),
        name: format!("Casque {sku}"),
        brand: Brand {
            name: "Sony".into(),
            slug: "sony".into(),
        },
        category_path: vec!["Audio".into()],
        short_description: String::new(),
        warranty_months: 24,
    }
}

fn family(name: &str) -> CreateFamily {
    CreateFamily {
        name: name.into(),
        slug: None,
    }
}

fn at(colour: &str, capacity: &str) -> Vec<OptionValue> {
    vec![
        OptionValue::new("Couleur", colour),
        OptionValue::new("Capacité", capacity),
    ]
}

fn colours_and_capacities() -> Vec<FamilyOption> {
    vec![
        FamilyOption::new("Couleur", &["Noir", "Argent", "Bleu"]),
        FamilyOption::new("Capacité", &["64 Go", "128 Go"]),
    ]
}

#[tokio::test]
async fn a_family_is_opened_told_apart_and_dissolved() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let id = cmd.create_family(family("Baladeur NW-A")).await?;
    assert_eq!(id, family_id("baladeur-nw-a"));
    assert!(matches!(
        cmd.create_family(family("Baladeur NW A")).await,
        Err(CatalogError::FamilySlugAlreadyExists(slug)) if slug == "baladeur-nw-a"
    ));
    assert!(matches!(
        cmd.create_family(family("  ")).await,
        Err(CatalogError::Required("name"))
    ));
    assert!(matches!(
        cmd.create_family(CreateFamily {
            name: "Autre".into(),
            slug: Some("Pas Un Slug".into()),
        })
        .await,
        Err(CatalogError::InvalidSlug(_))
    ));

    // Options are tidied; the same list records nothing.
    cmd.define_family_options(
        &id,
        vec![
            FamilyOption::new(" Couleur ", &["Noir", " Argent", "Noir", "", "Bleu"]),
            FamilyOption::new("", &["perdu"]),
            FamilyOption::new("Capacité", &["64 Go", "128 Go"]),
        ],
    )
    .await?;
    let state = cmd.load_family(&id).await?.unwrap_or_default();
    assert_eq!(state.options, colours_and_capacities());
    cmd.define_family_options(&id, colours_and_capacities())
        .await?;
    assert!(matches!(
        cmd.define_family_options(
            &id,
            vec![
                FamilyOption::new("Couleur", &["Noir"]),
                FamilyOption::new("Couleur", &["Bleu"]),
            ],
        )
        .await,
        Err(CatalogError::DuplicateOption(name)) if name == "Couleur"
    ));
    let too_many = (0..=MAX_FAMILY_OPTIONS)
        .map(|n| FamilyOption::new(format!("Option {n}"), &["a"]))
        .collect();
    assert!(matches!(
        cmd.define_family_options(&id, too_many).await,
        Err(CatalogError::TooManyOptions(MAX_FAMILY_OPTIONS))
    ));

    cmd.rename_family(&id, "Baladeurs NW-A").await?;
    sync(&executor, &db).await?;
    let row = family_by_id(&db, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no family row"))?;
    assert_eq!(row.name, "Baladeurs NW-A");
    assert_eq!(row.slug, "baladeur-nw-a");
    assert_eq!(row.option_list(), colours_and_capacities());
    assert_eq!(row.variant_count, 0);

    cmd.dissolve_family(&id).await?;
    cmd.dissolve_family(&id).await?;
    assert!(matches!(
        cmd.rename_family(&id, "Trop tard").await,
        Err(CatalogError::FamilyDissolved)
    ));
    sync(&executor, &db).await?;
    assert!(list_families(&db, false).await?.is_empty());
    assert_eq!(list_families(&db, true).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn products_take_their_place_in_one_family() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);
    let id = cmd.create_family(family("Baladeur NW-A")).await?;
    let black_64 = cmd.create_product(product("NWA-N-64")).await?;
    let black_128 = cmd.create_product(product("NWA-N-128")).await?;
    let silver_64 = cmd.create_product(product("NWA-A-64")).await?;

    // No options yet: nowhere to stand.
    assert!(matches!(
        cmd.place_variant(&id, &black_64, at("Noir", "64 Go")).await,
        Err(CatalogError::Required("options"))
    ));
    cmd.define_family_options(&id, colours_and_capacities())
        .await?;

    assert!(matches!(
        cmd.place_variant(&id, &black_64, vec![OptionValue::new("Couleur", "Noir")])
            .await,
        Err(CatalogError::MissingOptionValue(option)) if option == "Capacité"
    ));
    assert!(matches!(
        cmd.place_variant(&id, &black_64, at("Rose", "64 Go")).await,
        Err(CatalogError::UnknownOptionValue { option, value }) if option == "Couleur" && value == "Rose"
    ));
    assert!(matches!(
        cmd.place_variant(&id, "no-such-product", at("Noir", "64 Go"))
            .await,
        Err(CatalogError::ProductNotFound)
    ));

    // Placed — in the options' order whatever the order given —, placed again
    // (nothing), then moved.
    let reversed = vec![
        OptionValue::new("Capacité", "64 Go"),
        OptionValue::new("Couleur", "Noir"),
        OptionValue::new("Matière", "ignorée"),
    ];
    assert!(cmd.place_variant(&id, &black_64, reversed).await?);
    assert!(
        !cmd.place_variant(&id, &black_64, at("Noir", "64 Go"))
            .await?
    );
    assert!(
        cmd.place_variant(&id, &black_128, at("Noir", "128 Go"))
            .await?
    );
    assert!(matches!(
        cmd.place_variant(&id, &silver_64, at("Noir", "64 Go"))
            .await,
        Err(CatalogError::VariantPlaceTaken)
    ));
    assert!(
        cmd.place_variant(&id, &silver_64, at("Bleu", "64 Go"))
            .await?
    );
    assert!(
        cmd.place_variant(&id, &silver_64, at("Argent", "64 Go"))
            .await?
    );

    let state = cmd.load_family(&id).await?.unwrap_or_default();
    assert_eq!(state.variants.len(), 3);
    assert_eq!(state.variants[0].values, at("Noir", "64 Go"));
    assert_eq!(state.variants[2].values, at("Argent", "64 Go"));
    assert!(
        state
            .variants
            .iter()
            .all(|variant| state.is_complete(variant))
    );
    let page = load_product_page(&executor, &black_64)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no page"))?;
    assert_eq!(page.family_id.as_deref(), Some(id.as_str()));

    // One family at a time.
    let other = cmd.create_family(family("Autre famille")).await?;
    cmd.define_family_options(&other, colours_and_capacities())
        .await?;
    assert!(matches!(
        cmd.place_variant(&other, &black_64, at("Noir", "64 Go"))
            .await,
        Err(CatalogError::ProductInAnotherFamily)
    ));

    // What a variant stands on cannot be taken away; the rest can, and an
    // option can be added — the variants are then left to complete.
    assert!(matches!(
        cmd.define_family_options(&id, vec![FamilyOption::new("Couleur", &["Noir", "Argent"])])
            .await,
        Err(CatalogError::OptionInUse { option, .. }) if option == "Capacité"
    ));
    let mut wider = vec![
        FamilyOption::new("Couleur", &["Argent", "Noir"]),
        FamilyOption::new("Capacité", &["64 Go", "128 Go"]),
        FamilyOption::new("Édition", &["Standard", "Signature"]),
    ];
    cmd.define_family_options(&id, wider.clone()).await?;
    let state = cmd.load_family(&id).await?.unwrap_or_default();
    assert!(!state.is_complete(&state.variants[0]));
    wider.pop();
    cmd.define_family_options(&id, wider).await?;

    sync(&executor, &db).await?;
    let rows = family_variants(&db, &id).await?;
    assert_eq!(
        rows.iter()
            .map(|row| row.product_id.as_str())
            .collect::<Vec<_>>(),
        [black_64.as_str(), black_128.as_str(), silver_64.as_str()]
    );
    assert_eq!(rows[2].values(), at("Argent", "64 Go"));
    let found = family_of_product(&db, &black_128).await?;
    assert_eq!(
        found.map(|row| (row.id, row.variant_count)),
        Some((id.clone(), 3))
    );

    // Out of the family, free to join another; a family that holds variants
    // stays.
    assert!(matches!(
        cmd.dissolve_family(&id).await,
        Err(CatalogError::FamilyNotEmpty)
    ));
    assert!(cmd.remove_variant(&id, &black_64).await?);
    assert!(!cmd.remove_variant(&id, &black_64).await?);
    let page = load_product_page(&executor, &black_64)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no page"))?;
    assert_eq!(page.family_id, None);
    assert!(
        cmd.place_variant(&other, &black_64, at("Noir", "64 Go"))
            .await?
    );
    sync(&executor, &db).await?;
    assert_eq!(family_variants(&db, &id).await?.len(), 2);
    assert_eq!(
        family_of_product(&db, &black_64).await?.map(|row| row.id),
        Some(other)
    );

    // An archived product joins nothing.
    let gone = cmd.create_product(product("NWA-B-64")).await?;
    cmd.archive_product(&gone).await?;
    assert!(matches!(
        cmd.place_variant(&id, &gone, at("Bleu", "64 Go")).await,
        Err(CatalogError::ProductArchived)
    ));
    Ok(())
}

#[tokio::test]
async fn a_product_page_leads_to_the_closest_sibling() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);
    let id = cmd.create_family(family("Baladeur NW-A")).await?;
    cmd.define_family_options(&id, colours_and_capacities())
        .await?;
    let mut ids = Vec::new();
    for (sku, colour, capacity) in [
        ("N-64", "Noir", "64 Go"),
        ("N-128", "Noir", "128 Go"),
        ("A-64", "Argent", "64 Go"),
        ("B-128", "Bleu", "128 Go"),
    ] {
        let product_id = cmd.create_product(product(sku)).await?;
        cmd.place_variant(&id, &product_id, at(colour, capacity))
            .await?;
        ids.push(product_id);
    }
    let [black_64, black_128, silver_64, blue_128] = [&ids[0], &ids[1], &ids[2], &ids[3]];
    let state = cmd.load_family(&id).await?.unwrap_or_default();

    // From the black 64 Go, everything on sale.
    let choices = variant_choices(&state, black_64, |_| true);
    assert_eq!(choices.len(), 2);
    let colours = &choices[0];
    assert_eq!(colours.option, "Couleur");
    let seen: Vec<_> = colours
        .values
        .iter()
        .map(|v| {
            (
                v.value.as_str(),
                v.product_id.as_deref(),
                v.current,
                v.exact,
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("Noir", Some(black_64.as_str()), true, true),
            ("Argent", Some(silver_64.as_str()), false, true),
            // No blue 64 Go: blue leads to the 128 Go, and says so.
            ("Bleu", Some(blue_128.as_str()), false, false),
        ]
    );
    let capacities: Vec<_> = choices[1]
        .values
        .iter()
        .map(|v| (v.value.as_str(), v.product_id.as_deref(), v.exact))
        .collect();
    assert_eq!(
        capacities,
        [
            ("64 Go", Some(black_64.as_str()), true),
            ("128 Go", Some(black_128.as_str()), true),
        ]
    );

    // The silver one off sale: the colour leads nowhere; a page still shows
    // where it stands itself.
    let choices = variant_choices(&state, black_64, |id| id != silver_64.as_str());
    assert_eq!(choices[0].values[1].product_id, None);
    let choices = variant_choices(&state, silver_64, |id| id != silver_64.as_str());
    assert!(choices[0].values[1].current);
    assert_eq!(
        choices[0].values[1].product_id.as_deref(),
        Some(silver_64.as_str())
    );

    // Alone on sale, or not of the family: nothing to choose.
    assert!(variant_choices(&state, black_64, |_| false).is_empty());
    assert!(variant_choices(&state, "someone-else", |_| true).is_empty());
    Ok(())
}
