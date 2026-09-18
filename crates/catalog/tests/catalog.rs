use timada_catalog::{
    Brand, CatalogError, Command, CreateProduct, DescribeProduct, EnergyClass, ListProducts, Media,
    MediaKind, Spec, count_products, list_by_brand, list_products, load_product_page, migrations,
    product_list_subscription,
};

fn aoc_monitor() -> CreateProduct {
    CreateProduct {
        sku: "aoc-24g4xe".into(),
        name: "AOC 23.8\" LED - 24G4XE".into(),
        brand: Brand {
            name: "AOC".into(),
            slug: "aoc".into(),
        },
        category_path: vec![
            "Informatique".into(),
            "Périphériques".into(),
            "Ecran ordinateur".into(),
            "Ecran PC".into(),
        ],
        short_description: "Ecran PC Full HD 1080p - 180 Hz - 1 ms".into(),
        warranty_months: 60,
    }
}

#[tokio::test]
async fn product_page_reflects_commands() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let id = cmd.create_product(aoc_monitor()).await?;
    cmd.describe_product(
        &id,
        DescribeProduct {
            long_description: "Construisez vos victoires !".into(),
            key_features: vec!["Écran IPS de 23.8 pouces".into(), "180 Hz".into()],
        },
    )
    .await?;
    cmd.specify_product(
        &id,
        vec![Spec {
            group: "Dalle".into(),
            label: "Taille".into(),
            value: "23.8\"".into(),
        }],
    )
    .await?;
    cmd.add_product_media(
        &id,
        Media {
            url: "https://cdn.example/aoc-24g4xe.jpg".into(),
            kind: MediaKind::Image,
            alt: "AOC 24G4XE".into(),
        },
    )
    .await?;
    cmd.label_product_energy(&id, EnergyClass::E, "https://cdn.example/fiche.pdf".into())
        .await?;

    let view = load_product_page(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("product page missing"))?;
    assert_eq!(view.sku, "AOC-24G4XE");
    assert_eq!(view.brand.slug, "aoc");
    assert_eq!(view.category_path.len(), 4);
    assert_eq!(view.key_features.len(), 2);
    assert_eq!(view.specs.len(), 1);
    assert_eq!(view.media.len(), 1);
    assert_eq!(view.energy_class, Some(EnergyClass::E));
    assert_eq!(view.warranty_months, 60);
    assert!(!view.archived);

    product_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let rows = list_by_brand(&db, "aoc").await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert_eq!(list_products(&db, &ListProducts::default()).await?.len(), 1);
    let iiyama = ListProducts {
        q: Some("iiyama".into()),
        ..ListProducts::default()
    };
    assert!(list_products(&db, &iiyama).await?.is_empty());
    let by_sku = ListProducts {
        q: Some("24g4".into()),
        ..ListProducts::default()
    };
    assert_eq!(list_products(&db, &by_sku).await?.len(), 1);
    assert_eq!(count_products(&db, None, false).await?, 1);
    assert_eq!(
        rows[0].category_path,
        "Informatique > Périphériques > Ecran ordinateur > Ecran PC"
    );

    Ok(())
}

#[tokio::test]
async fn sku_is_unique_and_archiving_is_final() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let id = cmd.create_product(aoc_monitor()).await?;
    let duplicate = cmd.create_product(aoc_monitor()).await;
    assert!(matches!(duplicate, Err(CatalogError::SkuAlreadyExists(sku)) if sku == "AOC-24G4XE"));

    cmd.archive_product(&id).await?;
    let again = cmd.archive_product(&id).await;
    assert!(matches!(again, Err(CatalogError::ProductArchived)));

    product_list_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    assert!(list_by_brand(&db, "aoc").await?.is_empty());
    assert!(
        list_products(&db, &ListProducts::default())
            .await?
            .is_empty()
    );
    let with_archived = ListProducts {
        include_archived: true,
        ..ListProducts::default()
    };
    assert_eq!(list_products(&db, &with_archived).await?.len(), 1);
    assert_eq!(count_products(&db, None, true).await?, 1);

    Ok(())
}
