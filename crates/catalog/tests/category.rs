use evento::Executor;
use sqlx::SqlitePool;
use timada_catalog::{
    Brand, CatalogError, Command, CreateCategory, CreateProduct, MAX_CATEGORY_DEPTH,
    adopt_category_paths, category_by_slug, category_id, category_lineage,
    category_list_subscription, category_subtree_ids, category_tree, count_products_in_categories,
    is_on_storefront, list_categories, load_product_page, migrations, product_counts_by_category,
    product_list_subscription, products_in_categories,
};

async fn sync<E: Executor + Clone + 'static>(executor: &E, db: &SqlitePool) -> anyhow::Result<()> {
    category_list_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    product_list_subscription()
        .data(db.clone())
        .run_once(executor)
        .await?;
    Ok(())
}

fn category(name: &str, parent_id: Option<&str>) -> CreateCategory {
    CreateCategory {
        name: name.into(),
        slug: None,
        parent_id: parent_id.map(str::to_owned),
    }
}

fn product(sku: &str, path: &[&str]) -> CreateProduct {
    CreateProduct {
        sku: sku.into(),
        name: format!("Produit {sku}"),
        brand: Brand {
            name: "AOC".into(),
            slug: "aoc".into(),
        },
        category_path: path.iter().map(|s| (*s).to_owned()).collect(),
        short_description: String::new(),
        warranty_months: 24,
    }
}

/// `name (slug)` lines, indented by depth: the tree at a glance.
fn outline(nodes: &[timada_catalog::CategoryNode]) -> Vec<String> {
    nodes
        .iter()
        .flat_map(|node| node.flatten(0))
        .map(|(depth, c)| format!("{}{} ({})", "  ".repeat(depth), c.name, c.slug))
        .collect()
}

#[tokio::test]
async fn a_tree_is_built_renamed_reordered_and_pruned() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let computing = cmd.create_category(category("Informatique", None)).await?;
    assert_eq!(computing, category_id("informatique"));
    let screens = cmd
        .create_category(category("Écrans PC", Some(&computing)))
        .await?;
    let keyboards = cmd
        .create_category(category("Claviers", Some(&computing)))
        .await?;
    let gaming = cmd
        .create_category(CreateCategory {
            name: "Écrans gamer".into(),
            slug: Some("ecrans-gaming".into()),
            parent_id: Some(screens.clone()),
        })
        .await?;
    let home = cmd.create_category(category("Maison", None)).await?;

    // The slug is the category's identity: taken once, well formed, and
    // needing a parent that exists.
    assert!(matches!(
        cmd.create_category(category("informatique", None)).await,
        Err(CatalogError::SlugAlreadyExists(slug)) if slug == "informatique"
    ));
    let shouting = CreateCategory {
        slug: Some("Écrans".into()),
        ..category("Écrans", None)
    };
    assert!(matches!(
        cmd.create_category(shouting).await,
        Err(CatalogError::InvalidSlug(_))
    ));
    assert!(matches!(
        cmd.create_category(category("  ", None)).await,
        Err(CatalogError::Required("name"))
    ));
    assert!(matches!(
        cmd.create_category(category("→", None)).await,
        Err(CatalogError::Required("slug"))
    ));
    assert!(matches!(
        cmd.create_category(category("Orpheline", Some("nowhere")))
            .await,
        Err(CatalogError::CategoryNotFound)
    ));

    sync(&executor, &db).await?;
    let tree = category_tree(list_categories(&db, false).await?, false);
    assert_eq!(
        outline(&tree),
        [
            "Informatique (informatique)",
            "  Claviers (claviers)",
            "  Écrans PC (ecrans-pc)",
            "    Écrans gamer (ecrans-gaming)",
            "Maison (maison)",
        ]
    );

    // Renamed, described, put first: the slug does not move.
    cmd.rename_category(&screens, "Moniteurs").await?;
    cmd.describe_category(&screens, " Du bureau au jeu. ")
        .await?;
    cmd.position_category(&keyboards, 5).await?;
    sync(&executor, &db).await?;
    let row = category_by_slug(&db, "ecrans-pc")
        .await?
        .ok_or_else(|| anyhow::anyhow!("category missing"))?;
    assert_eq!(
        (row.name.as_str(), row.description.as_str()),
        ("Moniteurs", "Du bureau au jeu.")
    );
    let tree = category_tree(list_categories(&db, false).await?, false);
    assert_eq!(outline(&tree)[1], "  Moniteurs (ecrans-pc)");

    // The way down to a category, and everything under one.
    let lineage = category_lineage(&db, &gaming).await?;
    let names: Vec<&str> = lineage.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Informatique", "Moniteurs", "Écrans gamer"]);
    assert!(is_on_storefront(&lineage));
    let mut under = category_subtree_ids(&db, &computing).await?;
    under.sort();
    let mut expected = vec![
        computing.clone(),
        screens.clone(),
        keyboards.clone(),
        gaming.clone(),
    ];
    expected.sort();
    assert_eq!(under, expected);
    assert!(category_lineage(&db, "nowhere").await?.is_empty());

    // A branch moves with what is under it — never under itself.
    assert!(matches!(
        cmd.move_category(&computing, Some(gaming.clone())).await,
        Err(CatalogError::CategoryCycle)
    ));
    assert!(matches!(
        cmd.move_category(&screens, Some(screens.clone())).await,
        Err(CatalogError::CategoryCycle)
    ));
    cmd.move_category(&screens, Some(home.clone())).await?;
    cmd.move_category(&keyboards, None).await?;
    sync(&executor, &db).await?;
    let tree = category_tree(list_categories(&db, false).await?, false);
    assert_eq!(
        outline(&tree),
        [
            "Informatique (informatique)",
            "Maison (maison)",
            "  Moniteurs (ecrans-pc)",
            "    Écrans gamer (ecrans-gaming)",
            "Claviers (claviers)",
        ]
    );

    // Archived: gone from the storefront with its branch, still there for
    // the operator; nothing more can be done to it, twice is harmless.
    cmd.archive_category(&screens).await?;
    cmd.archive_category(&screens).await?;
    assert!(matches!(
        cmd.rename_category(&screens, "Écrans").await,
        Err(CatalogError::CategoryArchived)
    ));
    assert!(matches!(
        cmd.create_category(category("Écrans 4K", Some(&screens)))
            .await,
        Err(CatalogError::CategoryArchived)
    ));
    sync(&executor, &db).await?;
    let all = list_categories(&db, true).await?;
    assert_eq!(
        outline(&category_tree(all.clone(), false)),
        [
            "Informatique (informatique)",
            "Maison (maison)",
            "Claviers (claviers)",
        ]
    );
    assert_eq!(outline(&category_tree(all, true)).len(), 5);
    assert!(!is_on_storefront(&category_lineage(&db, &gaming).await?));
    assert_eq!(
        category_subtree_ids(&db, &home).await?,
        std::slice::from_ref(&home)
    );
    Ok(())
}

#[tokio::test]
async fn the_tree_only_goes_so_deep() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);
    let mut parent: Option<String> = None;
    for level in 1..=MAX_CATEGORY_DEPTH {
        let created = cmd
            .create_category(category(&format!("Niveau {level}"), parent.as_deref()))
            .await?;
        parent = Some(created);
    }
    assert!(matches!(
        cmd.create_category(category("Trop profond", parent.as_deref()))
            .await,
        Err(CatalogError::CategoryTooDeep(MAX_CATEGORY_DEPTH))
    ));
    // Nor by moving a category down there.
    let loose = cmd.create_category(category("Ailleurs", None)).await?;
    assert!(matches!(
        cmd.move_category(&loose, parent).await,
        Err(CatalogError::CategoryTooDeep(_))
    ));
    Ok(())
}

#[tokio::test]
async fn products_are_filed_under_categories() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);
    let computing = cmd.create_category(category("Informatique", None)).await?;
    let screens = cmd
        .create_category(category("Écrans PC", Some(&computing)))
        .await?;
    let old = cmd.create_category(category("Fin de série", None)).await?;
    cmd.archive_category(&old).await?;

    let monitor = cmd.create_product(product("aoc-1", &[])).await?;
    let cable = cmd.create_product(product("cab-1", &[])).await?;
    assert!(cmd.categorise_product(&monitor, &screens).await?);
    // Where it already is: nothing to record.
    assert!(!cmd.categorise_product(&monitor, &screens).await?);
    assert!(cmd.categorise_product(&cable, &computing).await?);
    assert!(matches!(
        cmd.categorise_product(&cable, &old).await,
        Err(CatalogError::CategoryArchived)
    ));
    assert!(matches!(
        cmd.categorise_product(&cable, "nowhere").await,
        Err(CatalogError::CategoryNotFound)
    ));
    assert!(matches!(
        cmd.categorise_product("nothing", &screens).await,
        Err(CatalogError::ProductNotFound)
    ));

    sync(&executor, &db).await?;
    let page = load_product_page(&executor, &monitor)
        .await?
        .ok_or_else(|| anyhow::anyhow!("product missing"))?;
    assert_eq!(page.category_id.as_deref(), Some(screens.as_str()));

    // A category's products span what is under it.
    let under = category_subtree_ids(&db, &computing).await?;
    let listed = products_in_categories(&db, &under, 10, 0).await?;
    let skus: Vec<&str> = listed.iter().map(|p| p.sku.as_str()).collect();
    assert_eq!(skus, ["AOC-1", "CAB-1"]);
    assert_eq!(count_products_in_categories(&db, &under).await?, 2);
    let only_screens = products_in_categories(&db, std::slice::from_ref(&screens), 10, 0).await?;
    assert_eq!(only_screens.len(), 1);
    assert_eq!(
        only_screens[0].category_id.as_deref(),
        Some(screens.as_str())
    );
    assert!(products_in_categories(&db, &[], 10, 0).await?.is_empty());

    // It moves; an archived product no longer counts.
    cmd.categorise_product(&monitor, &computing).await?;
    cmd.archive_product(&cable).await?;
    sync(&executor, &db).await?;
    let counts = product_counts_by_category(&db).await?;
    assert_eq!(counts.get(&computing), Some(&1));
    assert_eq!(counts.get(&screens), None);
    Ok(())
}

#[tokio::test]
async fn category_paths_from_before_are_adopted() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);
    let monitor = cmd
        .create_product(product(
            "aoc-1",
            &["Informatique", "Périphériques", "Écran PC"],
        ))
        .await?;
    let keyboard = cmd
        .create_product(product("kbd-1", &["Informatique", "Périphériques"]))
        .await?;
    // The same word under another branch.
    let lamp = cmd
        .create_product(product("lmp-1", &["Maison", "Périphériques"]))
        .await?;
    let loose = cmd.create_product(product("msc-1", &[])).await?;
    // Filed by hand already: left where it is.
    let manual = cmd.create_category(category("Sélection", None)).await?;
    let picked = cmd
        .create_product(product("pck-1", &["Informatique"]))
        .await?;
    cmd.categorise_product(&picked, &manual).await?;
    sync(&executor, &db).await?;

    assert_eq!(adopt_category_paths(&executor, &db).await?, 3);
    sync(&executor, &db).await?;
    let tree = category_tree(list_categories(&db, false).await?, false);
    assert_eq!(
        outline(&tree),
        [
            "Informatique (informatique)",
            "  Périphériques (peripheriques)",
            "    Écran PC (ecran-pc)",
            "Maison (maison)",
            "  Périphériques (maison-peripheriques)",
            "Sélection (selection)",
        ]
    );
    let filed_under = |id: String| {
        let executor = &executor;
        async move {
            let page = load_product_page(executor, &id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("product missing"))?;
            Ok::<_, anyhow::Error>(page.category_id)
        }
    };
    assert_eq!(filed_under(monitor).await?, Some(category_id("ecran-pc")));
    assert_eq!(
        filed_under(keyboard).await?,
        Some(category_id("peripheriques"))
    );
    assert_eq!(
        filed_under(lamp).await?,
        Some(category_id("maison-peripheriques"))
    );
    assert_eq!(filed_under(loose).await?, None);
    assert_eq!(filed_under(picked).await?, Some(manual));

    // Again: nothing left to adopt, nothing opened twice.
    assert_eq!(adopt_category_paths(&executor, &db).await?, 0);
    sync(&executor, &db).await?;
    assert_eq!(list_categories(&db, true).await?.len(), 6);
    Ok(())
}

#[tokio::test]
async fn a_category_is_filtered_by_its_own_specs_or_its_parents() -> anyhow::Result<()> {
    use timada_catalog::{MAX_CATEGORY_FACETS, SpecKey, effective_facets};

    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);
    let computing = cmd.create_category(category("Informatique", None)).await?;
    let screens = cmd
        .create_category(category("Écrans PC", Some(&computing)))
        .await?;
    let gaming = cmd
        .create_category(category("Écrans gamer", Some(&screens)))
        .await?;

    let size = SpecKey::new("Dalle", "Taille");
    let panel = SpecKey::new("Dalle", "Type");
    // Tidied: trimmed, blanks and repeats dropped, the order kept.
    cmd.define_category_facets(
        &screens,
        vec![
            SpecKey::new(" Dalle ", " Taille "),
            panel.clone(),
            SpecKey::new("Dalle", "  "),
            size.clone(),
        ],
    )
    .await?;
    sync(&executor, &db).await?;
    let facets_of = |id: String| {
        let db = &db;
        async move { Ok::<_, anyhow::Error>(effective_facets(&category_lineage(db, &id).await?)) }
    };
    assert_eq!(
        facets_of(screens.clone()).await?,
        [size.clone(), panel.clone()]
    );
    // Inherited below, nothing above.
    assert_eq!(
        facets_of(gaming.clone()).await?,
        [size.clone(), panel.clone()]
    );
    assert!(facets_of(computing.clone()).await?.is_empty());

    // A list of its own replaces the inherited one; emptied, it is inherited again.
    let refresh = SpecKey::new("Dalle", "Fréquence");
    cmd.define_category_facets(&gaming, vec![refresh.clone()])
        .await?;
    sync(&executor, &db).await?;
    assert_eq!(facets_of(gaming.clone()).await?, [refresh]);
    cmd.define_category_facets(&gaming, Vec::new()).await?;
    sync(&executor, &db).await?;
    assert_eq!(facets_of(gaming).await?, [size, panel]);

    let too_many = (0..=MAX_CATEGORY_FACETS)
        .map(|n| SpecKey::new("Dalle", format!("Spec {n}")))
        .collect();
    assert!(matches!(
        cmd.define_category_facets(&screens, too_many).await,
        Err(CatalogError::TooManyFacets(_))
    ));
    Ok(())
}
