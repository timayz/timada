//! What the storefront lists: rows that follow the four contexts, full-text
//! search, filters with their facet counts, and sorting.

use evento::Executor;
use sqlx::SqlitePool;
use timada_catalog::{
    Brand, Command, CreateCategory, CreateProduct, DescribeProduct, ListingQuery, ListingSort,
    Media, MediaKind, brand_by_slug, listed_products, listing_subscription, migrations,
    search_listing,
};
use timada_core::Money;
use timada_inventory::{RegisterStockItem, StockLocation};

struct Shop {
    executor: evento::Sqlite,
    db: SqlitePool,
}

impl Shop {
    async fn open() -> anyhow::Result<Self> {
        let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
        Ok(Self { executor, db })
    }

    async fn sync(&self) -> anyhow::Result<()> {
        listing_subscription()
            .data(self.db.clone())
            .run_once(&self.executor)
            .await
    }

    /// A product on sale: created, priced, `stock` units in the warehouse.
    async fn sell(
        &self,
        sku: &str,
        name: &str,
        brand: &str,
        category_id: Option<&str>,
        cents: i64,
        stock: u32,
    ) -> anyhow::Result<String> {
        let id = Command(&self.executor)
            .create_product(CreateProduct {
                sku: sku.into(),
                name: name.into(),
                brand: Brand {
                    name: brand.into(),
                    slug: timada_core::slug::slugify(brand),
                },
                category_path: Vec::new(),
                short_description: String::new(),
                warranty_months: 24,
            })
            .await?;
        if let Some(category_id) = category_id {
            Command(&self.executor)
                .categorise_product(&id, category_id)
                .await?;
        }
        timada_pricing::Command(&self.executor)
            .list_price(timada_pricing::ListPrice {
                product_id: id.clone(),
                price_incl_tax: Money::eur(cents),
                vat_rate_bp: 2_000,
                eco_participation: Money::eur(0),
            })
            .await?;
        let inventory = timada_inventory::Command(&self.executor);
        let item = inventory
            .register_stock_item(RegisterStockItem {
                product_id: id.clone(),
                location: StockLocation::Warehouse,
            })
            .await?;
        if stock > 0 {
            inventory.receive_stock(&item, stock).await?;
        }
        Ok(id)
    }

    async fn review(&self, product_id: &str, customer: &str, rating: u8) -> anyhow::Result<()> {
        let reviews = timada_review::Command(&self.executor);
        let id = reviews
            .submit_review(timada_review::SubmitReview {
                product_id: product_id.into(),
                customer_id: customer.into(),
                order_id: None,
                rating,
                title: "Avis".into(),
                body: "Un avis assez long pour être accepté.".into(),
            })
            .await?;
        reviews.publish_review(&id).await?;
        Ok(())
    }

    async fn skus(&self, query: &ListingQuery) -> anyhow::Result<Vec<String>> {
        Ok(search_listing(&self.db, query)
            .await?
            .rows
            .into_iter()
            .map(|row| row.sku)
            .collect())
    }
}

async fn category<E: Executor>(
    executor: &E,
    name: &str,
    parent_id: Option<&str>,
) -> anyhow::Result<String> {
    Ok(Command(executor)
        .create_category(CreateCategory {
            name: name.into(),
            slug: None,
            parent_id: parent_id.map(str::to_owned),
        })
        .await?)
}

/// Two monitors, a keyboard and a lamp, across two brands and two branches.
async fn stocked_shop() -> anyhow::Result<(Shop, [String; 4], [String; 3])> {
    let shop = Shop::open().await?;
    let computing = category(&shop.executor, "Informatique", None).await?;
    let screens = category(&shop.executor, "Écrans PC", Some(&computing)).await?;
    let home = category(&shop.executor, "Maison", None).await?;
    let gamer = shop
        .sell(
            "AOC-24G",
            "AOC 24\" Gaming 180 Hz",
            "AOC",
            Some(&screens),
            11_995,
            5,
        )
        .await?;
    let office = shop
        .sell(
            "LG-27U",
            "LG 27\" UltraFine bureautique",
            "LG",
            Some(&screens),
            34_900,
            0,
        )
        .await?;
    let keyboard = shop
        .sell(
            "LG-KB1",
            "Clavier mécanique",
            "LG",
            Some(&computing),
            7_990,
            12,
        )
        .await?;
    let lamp = shop
        .sell(
            "LMP-1",
            "Lampe de bureau",
            "Maison & Co",
            Some(&home),
            2_990,
            3,
        )
        .await?;
    shop.sync().await?;
    Ok((
        shop,
        [gamer, office, keyboard, lamp],
        [computing, screens, home],
    ))
}

#[tokio::test]
async fn a_row_follows_what_the_four_contexts_say() -> anyhow::Result<()> {
    let (shop, [gamer, ..], _) = stocked_shop().await?;
    let find = || async {
        let page = search_listing(
            &shop.db,
            &ListingQuery {
                q: Some("AOC-24G".into()),
                ..ListingQuery::default()
            },
        )
        .await?;
        Ok::<_, anyhow::Error>(page.rows.into_iter().next())
    };
    let row = find().await?.ok_or_else(|| anyhow::anyhow!("not listed"))?;
    assert_eq!((row.price_minor, row.currency.as_str()), (11_995, "EUR"));
    assert_eq!((row.available, row.review_count), (5, 0));
    assert_eq!((row.rating_avg, row.thumbnail_url.as_deref()), (None, None));
    assert_eq!(row.brand_slug, "aoc");

    // Catalog, pricing, inventory and review each move — the row follows.
    let catalog = Command(&shop.executor);
    catalog
        .add_product_media(
            &gamer,
            Media {
                url: "/media/aoc.mp4".into(),
                kind: MediaKind::Video,
                alt: "Présentation".into(),
            },
        )
        .await?;
    catalog
        .add_product_media(
            &gamer,
            Media {
                url: "/media/aoc-front.avif".into(),
                kind: MediaKind::Image,
                alt: "AOC 24G de face".into(),
            },
        )
        .await?;
    timada_pricing::Command(&shop.executor)
        .change_price(timada_pricing::price_id(&gamer), Money::eur(9_995))
        .await?;
    let item = timada_inventory::stock_item_id(&gamer, &StockLocation::Warehouse);
    timada_inventory::Command(&shop.executor)
        .reserve_stock(&item, "order-1", 2)
        .await?;
    shop.review(&gamer, "ada", 5).await?;
    shop.review(&gamer, "bob", 4).await?;
    // A shop's own shelf is not what the warehouse delivers.
    let shelf = timada_inventory::Command(&shop.executor)
        .register_stock_item(RegisterStockItem {
            product_id: gamer.clone(),
            location: StockLocation::Store {
                store_id: "toulouse".into(),
            },
        })
        .await?;
    timada_inventory::Command(&shop.executor)
        .receive_stock(&shelf, 40)
        .await?;
    shop.sync().await?;
    // Redelivered or run again: the same row.
    shop.sync().await?;

    let row = find().await?.ok_or_else(|| anyhow::anyhow!("not listed"))?;
    assert_eq!(row.price_minor, 9_995);
    assert_eq!(row.available, 3);
    assert_eq!((row.rating_avg, row.review_count), (Some(4.5), 2));
    // The first *image*, not the video before it.
    assert_eq!(row.thumbnail_url.as_deref(), Some("/media/aoc-front.avif"));
    assert_eq!(row.thumbnail_alt.as_deref(), Some("AOC 24G de face"));

    // No price, no sale; archived, gone.
    timada_pricing::Command(&shop.executor)
        .withdraw_price(timada_pricing::price_id(&gamer))
        .await?;
    shop.sync().await?;
    assert_eq!(find().await?, None);
    assert_eq!(listed_products(&shop.db).await?.len(), 3);
    assert_eq!(brand_by_slug(&shop.db, "aoc").await?, None);
    assert_eq!(brand_by_slug(&shop.db, "lg").await?.as_deref(), Some("LG"));
    Ok(())
}

#[tokio::test]
async fn search_folds_accents_matches_prefixes_and_ranks_names_first() -> anyhow::Result<()> {
    let (shop, [gamer, ..], [_, screens, _]) = stocked_shop().await?;
    Command(&shop.executor)
        .describe_product(
            &gamer,
            DescribeProduct {
                long_description: String::new(),
                key_features: vec!["Dalle IPS idéale avec un clavier gamer".into()],
            },
        )
        .await?;
    shop.sync().await?;
    // Best match first.
    let ranked = |q: &str| {
        let query = ListingQuery {
            q: Some(q.into()),
            ..ListingQuery::default()
        };
        let shop = &shop;
        async move { shop.skus(&query).await }
    };
    // What matches, whatever the rank.
    let search = |q: &str| {
        let ranked = ranked(q);
        async move {
            let mut skus = ranked.await?;
            skus.sort();
            Ok::<_, anyhow::Error>(skus)
        }
    };

    // Accents and case do not matter; a word may be unfinished.
    assert_eq!(search("ecran").await?, ["AOC-24G", "LG-27U"]);
    assert_eq!(search("ÉCRANS").await?, ["AOC-24G", "LG-27U"]);
    assert_eq!(search("mecan").await?, ["LG-KB1"]);
    // Every word must match, wherever: brand + category, SKU, key features.
    assert_eq!(search("lg écran").await?, ["LG-27U"]);
    assert_eq!(search("lmp-1").await?, ["LMP-1"]);
    assert_eq!(search("ips").await?, ["AOC-24G"]);
    assert!(search("trottinette").await?.is_empty());
    // A name outranks a mention in the features.
    assert_eq!(ranked("clavier").await?, ["LG-KB1", "AOC-24G"]);
    // Nothing to search for is no search; operators are just characters.
    assert_eq!(search("  ").await?.len(), 4);
    assert_eq!(search("\"lampe\" OR NOT *").await?, Vec::<String>::new());
    assert_eq!(search("lampe)").await?, ["LMP-1"]);

    // A renamed category is searchable under its new name only.
    Command(&shop.executor)
        .rename_category(&screens, "Moniteurs")
        .await?;
    shop.sync().await?;
    assert_eq!(search("moniteur").await?, ["AOC-24G", "LG-27U"]);
    assert!(search("ecrans").await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn filters_narrow_and_facets_count_what_each_would_give() -> anyhow::Result<()> {
    let (shop, [gamer, office, ..], [computing, screens, home]) = stocked_shop().await?;
    shop.review(&gamer, "ada", 5).await?;
    shop.review(&office, "ada", 3).await?;
    shop.sync().await?;

    // A category spans its branch.
    let in_computing = ListingQuery {
        category_id: Some(computing.clone()),
        ..ListingQuery::default()
    };
    let page = search_listing(&shop.db, &in_computing).await?;
    assert_eq!(page.total, 3);
    let brands: Vec<(&str, i64)> = page
        .facets
        .brands
        .iter()
        .map(|b| (b.name.as_str(), b.count))
        .collect();
    assert_eq!(brands, [("AOC", 1), ("LG", 2)]);
    assert_eq!(page.facets.price_range, Some((7_990, 34_900)));
    assert_eq!(page.facets.in_stock, 2);
    assert_eq!(page.facets.rated_at_least, [(4, 1), (3, 2), (2, 2), (1, 2)]);

    // A brand picked: the other brands still say what they would add, while
    // the other facets count inside the pick.
    let lg = ListingQuery {
        brand_slugs: vec!["lg".into()],
        ..in_computing.clone()
    };
    let page = search_listing(&shop.db, &lg).await?;
    assert_eq!(page.total, 2);
    assert_eq!(page.facets.brands.len(), 2);
    assert_eq!(page.facets.in_stock, 1);
    assert_eq!(page.facets.price_range, Some((7_990, 34_900)));

    let narrowed = ListingQuery {
        in_stock: true,
        price_max_minor: Some(10_000),
        ..lg.clone()
    };
    assert_eq!(shop.skus(&narrowed).await?, ["LG-KB1"]);
    let page = search_listing(&shop.db, &narrowed).await?;
    // The price facet ignores the price filter, not the stock one.
    assert_eq!(page.facets.price_range, Some((7_990, 7_990)));

    let well_rated = ListingQuery {
        min_rating: Some(4),
        ..ListingQuery::default()
    };
    assert_eq!(shop.skus(&well_rated).await?, ["AOC-24G"]);
    let two_brands = ListingQuery {
        brand_slugs: vec!["aoc".into(), "maison-co".into()],
        ..ListingQuery::default()
    };
    assert_eq!(shop.skus(&two_brands).await?, ["AOC-24G", "LMP-1"]);

    // A branch that moves takes its products along.
    Command(&shop.executor)
        .move_category(&screens, Some(home.clone()))
        .await?;
    shop.sync().await?;
    assert_eq!(shop.skus(&in_computing).await?, ["LG-KB1"]);
    let in_home = ListingQuery {
        category_id: Some(home),
        ..ListingQuery::default()
    };
    assert_eq!(search_listing(&shop.db, &in_home).await?.total, 3);
    Ok(())
}

#[tokio::test]
async fn sorting_and_paging() -> anyhow::Result<()> {
    let (shop, [gamer, office, ..], _) = stocked_shop().await?;
    shop.review(&gamer, "ada", 4).await?;
    shop.review(&office, "ada", 4).await?;
    shop.review(&office, "bob", 4).await?;
    shop.sync().await?;
    let sorted = |sort: ListingSort| {
        let query = ListingQuery {
            sort,
            ..ListingQuery::default()
        };
        let shop = &shop;
        async move { shop.skus(&query).await }
    };

    assert_eq!(
        sorted(ListingSort::Relevance).await?,
        ["AOC-24G", "LG-KB1", "LMP-1", "LG-27U"],
        "by name without a search"
    );
    assert_eq!(
        sorted(ListingSort::PriceAsc).await?,
        ["LMP-1", "LG-KB1", "AOC-24G", "LG-27U"]
    );
    assert_eq!(
        sorted(ListingSort::PriceDesc).await?,
        ["LG-27U", "AOC-24G", "LG-KB1", "LMP-1"]
    );
    // Equal averages: the one more shoppers rated; the unrated last.
    assert_eq!(
        sorted(ListingSort::Rating).await?[..2],
        ["LG-27U", "AOC-24G"]
    );
    assert_eq!(
        sorted(ListingSort::Newest).await?,
        ["LMP-1", "LG-KB1", "LG-27U", "AOC-24G"]
    );

    let second_page = ListingQuery {
        sort: ListingSort::PriceAsc,
        limit: 3,
        offset: 3,
        ..ListingQuery::default()
    };
    let page = search_listing(&shop.db, &second_page).await?;
    assert_eq!(page.total, 4);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].sku, "LG-27U");
    Ok(())
}

fn spec(group: &str, label: &str, value: &str) -> timada_catalog::Spec {
    timada_catalog::Spec {
        group: group.into(),
        label: label.into(),
        value: value.into(),
    }
}

#[tokio::test]
async fn the_technical_sheet_filters_and_counts() -> anyhow::Result<()> {
    use timada_catalog::{SpecFacet, SpecFilter, SpecKey, specs_in_category};

    let (shop, [gamer, office, keyboard, _], [computing, screens, _]) = stocked_shop().await?;
    let catalog = Command(&shop.executor);
    catalog
        .specify_product(
            &gamer,
            vec![
                spec("Dalle", "Taille", "24 pouces"),
                spec("Dalle", "Type", "IPS"),
                spec("Dalle", "Fréquence", "180 Hz"),
            ],
        )
        .await?;
    catalog
        .specify_product(
            &office,
            vec![
                spec("Dalle", "Taille", "27 pouces"),
                spec("Dalle", "Type", "IPS"),
                spec("Dalle", "Fréquence", "60 Hz"),
                spec("Dalle", "Note", "  "),
            ],
        )
        .await?;
    catalog
        .specify_product(&keyboard, vec![spec("Touches", "Type", "Mécanique")])
        .await?;
    // A third monitor, another panel.
    let curved = shop
        .sell(
            "SAM-32C",
            "Samsung 32\" incurvé",
            "Samsung",
            Some(&screens),
            27_900,
            4,
        )
        .await?;
    catalog
        .specify_product(
            &curved,
            vec![
                spec("Dalle", "Taille", "32 pouces"),
                spec("Dalle", "Type", "VA"),
                spec("Dalle", "Fréquence", "144 Hz"),
            ],
        )
        .await?;
    shop.sync().await?;

    let size = SpecKey::new("Dalle", "Taille");
    let panel = SpecKey::new("Dalle", "Type");
    let refresh = SpecKey::new("Dalle", "Fréquence");
    let in_screens = ListingQuery {
        category_id: Some(screens.clone()),
        facet_specs: vec![
            panel.clone(),
            refresh.clone(),
            SpecKey::new("Dalle", "Poids"),
        ],
        ..ListingQuery::default()
    };
    let page = search_listing(&shop.db, &in_screens).await?;
    assert_eq!(
        page.facets.specs,
        [
            SpecFacet {
                key: panel.clone(),
                values: vec![("IPS".into(), 2), ("VA".into(), 1)],
            },
            // By their number, not the alphabet — and no facet for a spec
            // nobody has.
            SpecFacet {
                key: refresh.clone(),
                values: vec![
                    ("60 Hz".into(), 1),
                    ("144 Hz".into(), 1),
                    ("180 Hz".into(), 1)
                ],
            },
        ]
    );

    // Any of a spec's values, every spec: IPS *and* (144 or 180 Hz).
    let narrowed = ListingQuery {
        specs: vec![
            SpecFilter {
                key: panel.clone(),
                values: vec!["IPS".into()],
            },
            SpecFilter {
                key: refresh.clone(),
                values: vec!["144 Hz".into(), "180 Hz".into()],
            },
        ],
        ..in_screens.clone()
    };
    let page = search_listing(&shop.db, &narrowed).await?;
    let skus: Vec<&str> = page.rows.iter().map(|row| row.sku.as_str()).collect();
    assert_eq!(skus, ["AOC-24G"]);
    // Each facet counts inside the *other* picks: VA stays on offer among the
    // fast screens, 60 Hz among the IPS ones.
    assert_eq!(
        page.facets.specs[0].values,
        [("IPS".to_owned(), 1), ("VA".to_owned(), 1)]
    );
    assert_eq!(
        page.facets.specs[1].values,
        [("60 Hz".to_owned(), 1), ("180 Hz".to_owned(), 1)]
    );
    // The same label in another group is another spec.
    let keys = ListingQuery {
        specs: vec![SpecFilter {
            key: SpecKey::new("Touches", "Type"),
            values: vec!["Mécanique".into()],
        }],
        ..ListingQuery::default()
    };
    assert_eq!(shop.skus(&keys).await?, ["LG-KB1"]);

    // What an operator picks from: the specs of the branch, most common first.
    let offered = specs_in_category(&shop.db, &computing).await?;
    assert_eq!(offered[0].1, 3);
    assert!(offered.contains(&(size, 3)));
    assert!(offered.contains(&(SpecKey::new("Touches", "Type"), 1)));
    assert_eq!(offered.len(), 4, "a blank value is no spec: {offered:?}");

    // A sheet is replaced whole: what left it no longer matches.
    catalog
        .specify_product(&gamer, vec![spec("Dalle", "Type", "OLED")])
        .await?;
    shop.sync().await?;
    assert!(search_listing(&shop.db, &narrowed).await?.rows.is_empty());
    Ok(())
}
