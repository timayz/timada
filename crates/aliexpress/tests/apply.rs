use std::path::Path;
use std::sync::Arc;

use timada_aliexpress::{
    AeProduct, AeSku, AeSkuProperty, Fit, LinkState, LinkStatus, SyncRead, add_combinations,
    apply_sync, fit, map_product, track_imported,
};
use timada_catalog_import::{ImportOptions, NoImages, import_one};
use timada_tenancy::{Pricing, PricingRule, Rounding};

const ALICE: &str = "user-alice";
/// The tenant the tests act in.
const TENANT: &str = "acme";

async fn executor() -> evento::Turso {
    let store = evento::Turso::in_memory_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_catalog::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    store
}

fn fixture() -> AeProduct {
    let body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/product.json")).unwrap();
    AeProduct::from_response("1005006123456789", &body).unwrap()
}

const RULE: PricingRule = PricingRule {
    multiplier_bps: 20_000,
    plus_cents: 0,
    rounding: Rounding::Cent,
};
const PRICING: Pricing = Pricing {
    bands: Vec::new(),
    above: RULE,
};

#[tokio::test]
async fn an_import_makes_a_counted_draft_that_a_read_brings_in_step() {
    let executor = executor().await;
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);

    let link_id = links
        .request_import(ALICE, "1005006123456789")
        .await
        .unwrap();
    assert!(matches!(
        links.request_import(ALICE, "1005006123456789").await,
        Err(timada_aliexpress::AliexpressError::Importing)
    ));
    assert!(links.start_import("node-a", &link_id).await.unwrap());
    assert!(!links.start_import("node-b", &link_id).await.unwrap());

    let mapped = map_product(&fixture(), "EUR", Some(&PRICING)).unwrap();
    let imported = import_one(
        &catalog,
        ALICE,
        &mapped.seed,
        Path::new(""),
        Arc::new(NoImages),
        &ImportOptions {
            publish: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let skus = track_imported(
        &catalog,
        ALICE,
        &imported.product_id,
        &imported.variant_ids,
        &mapped.skus,
    )
    .await
    .unwrap();
    links
        .record_import(ALICE, &link_id, &imported.product_id, skus)
        .await
        .unwrap();

    let product = catalog
        .load_product(&imported.product_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!product.is_published());
    assert_eq!(product.variants.len(), 2);
    assert_eq!(product.variants[0].price.amount_cents, 862);
    let rose = imported.variant_ids[0].clone().unwrap();
    let stock = catalog
        .load_stock(&product.id, Some(&rose))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stock.on_hand, 120);
    catalog.publish_product(ALICE, &product.id).await.unwrap();

    // AliExpress sells fewer, dearer.
    let mut read = fixture();
    read.skus[0].available = 7;
    read.skus[0].cost_cents = Some(500);
    let (link, version) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    assert_eq!(link.status, LinkStatus::Linked);
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(!outcome.offline);
    assert_eq!(outcome.repriced, 1);
    links
        .record_sync(
            "sync",
            &link_id,
            version,
            SyncRead {
                skus: outcome.skus,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // A read that started from the same version lost the race.
    assert!(
        links
            .record_sync("sync", &link_id, version, SyncRead::default())
            .await
            .is_err()
    );
    let product = catalog.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(product.variant(&rose).unwrap().price.amount_cents, 1000);
    let stock = catalog
        .load_stock(&product.id, Some(&rose))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stock.on_hand, 7);
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    assert_eq!(link.skus[0].cost_cents, 500);

    // Withdrawn: nothing left to sell, and off sale.
    let mut read = fixture();
    read.status = "offline".into();
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(outcome.offline);
    assert!(outcome.unpublished);
    let product = catalog.load_product(&product.id).await.unwrap().unwrap();
    assert!(!product.is_published());
    let stock = catalog
        .load_stock(&product.id, Some(&rose))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stock.on_hand, 0);
}

#[tokio::test]
async fn one_node_renews_a_token() {
    let executor = executor().await;
    let claim = |actor: &'static str, company: &'static str, expires_at: u64| {
        let executor = executor.clone();
        async move {
            timada_aliexpress::claim_token_refresh(&executor, actor, company, expires_at)
                .await
                .unwrap()
        }
    };
    assert!(claim("node-a", "company-1", 1_700_000_000).await);
    assert!(!claim("node-b", "company-1", 1_700_000_000).await);
    // The next token is a renewal of its own, and so is another company's.
    assert!(claim("node-b", "company-1", 1_700_086_400).await);
    assert!(claim("node-b", "company-2", 1_700_000_000).await);
}

/// Imports the fixture as a draft; the link's id and the product's.
#[tokio::test]
async fn a_quote_in_another_currency_counts_the_stock_but_leaves_the_price() {
    let executor = executor().await;
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let (link_id, product_id) = imported(&executor).await;
    let product = catalog.load_product(&product_id).await.unwrap().unwrap();
    let rose = product.variants[0].id.clone();
    let price_before = product.variant(&rose).unwrap().price.amount_cents;

    // AliExpress answers the rose in dollars, and offers a new colour in
    // dollars too.
    let mut read = fixture();
    read.skus[0].available = 7;
    read.skus[0].cost_cents = Some(500);
    read.skus[0].currency = Some("usd".into());
    let mut green = sku("12000037009", 700, 3, &[("Color", "Green"), ("Size", "S")]);
    green.currency = Some("USD".into());
    read.skus.push(green);
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert_eq!(outcome.repriced, 0);
    assert!(outcome.found.is_empty());
    assert_eq!(outcome.warnings.len(), 2, "{:?}", outcome.warnings);
    assert!(outcome.warnings[0].contains("12000037001 is quoted in USD, not EUR"));
    assert!(outcome.warnings[1].contains("new combination 12000037009 is quoted in USD"));
    assert_eq!(outcome.skus[0].cost_cents, 431);
    let product = catalog.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(
        product.variant(&rose).unwrap().price.amount_cents,
        price_before
    );
    let stock = catalog
        .load_stock(&product_id, Some(&rose))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stock.on_hand, 7);
}

#[tokio::test]
async fn a_read_is_worth_a_record_only_when_the_link_holds_something_else() {
    let executor = executor().await;
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let (link_id, product_id) = imported(&executor).await;
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();

    // The same as the import: nothing to say.
    let outcome = apply_sync(&catalog, "sync", &link, Some(&fixture()), Some(&PRICING))
        .await
        .unwrap();
    assert!(!outcome.differs_from(&link));

    // AliExpress sold some: the catalog counts them, the link holds no count.
    let mut read = fixture();
    read.skus[0].available = 7;
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(!outcome.differs_from(&link));
    let product = catalog.load_product(&product_id).await.unwrap().unwrap();
    let stock = catalog
        .load_stock(&product_id, Some(&product.variants[0].id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stock.on_hand, 7);

    // A new combination is; how many of it there are, once known, is not.
    let mut read = fixture();
    read.skus
        .push(sku("12000037009", 700, 3, &[("Color", "Green")]));
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(outcome.found_differs_from(&link));
    assert!(outcome.differs_from(&link));
    let found = LinkState {
        found: outcome.found.clone(),
        ..link.clone()
    };
    let mut read = fixture();
    read.skus
        .push(sku("12000037009", 700, 1, &[("Color", "Green")]));
    let outcome = apply_sync(&catalog, "sync", &found, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(!outcome.differs_from(&found));
    let mut read = fixture();
    read.skus
        .push(sku("12000037009", 800, 1, &[("Color", "Green")]));
    let outcome = apply_sync(&catalog, "sync", &found, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(
        outcome.found_differs_from(&found),
        "it costs something else"
    );

    // The cost moved: the link holds what it cost.
    let mut read = fixture();
    read.skus[0].cost_cents = Some(500);
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(outcome.differs_from(&link));

    // AliExpress stopped selling it.
    let mut read = fixture();
    read.status = "offline".into();
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(outcome.differs_from(&link));
}

#[tokio::test]
async fn asking_for_a_read_takes_a_version_of_a_linked_item() {
    let executor = executor().await;
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);

    // Nothing to read: unknown, or still importing.
    assert!(matches!(
        links.request_sync(ALICE, "no-such-link").await,
        Err(timada_aliexpress::AliexpressError::NotFound)
    ));
    let importing = links
        .request_import(ALICE, "1005000000000009")
        .await
        .unwrap();
    assert!(matches!(
        links.request_sync(ALICE, &importing).await,
        Err(timada_aliexpress::AliexpressError::NotFound)
    ));

    let (link_id, _) = imported(&executor).await;
    let (_, version) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    let asked = links.request_sync(ALICE, &link_id).await.unwrap();
    assert_eq!(asked, version + 1);
    let (link, version) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    assert_eq!(version, asked);
    assert!(link.last_requested_at.is_some());
    // A read that started before the request lost its record to it.
    assert!(
        links
            .record_sync("sync", &link_id, asked - 1, SyncRead::default())
            .await
            .is_err()
    );

    // What a member waits for is told apart from what the dashboard reads.
    links
        .record_sync(
            timada_aliexpress::ALIEXPRESS_ACTOR,
            &link_id,
            asked,
            SyncRead::default(),
        )
        .await
        .unwrap();
    let (link, version) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    assert_eq!(link.answered_version, 0);
    links
        .record_sync(ALICE, &link_id, version, SyncRead::default())
        .await
        .unwrap();
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    assert_eq!(link.answered_version, version + 1);
    links
        .record_sync_failure(ALICE, &link_id, "no")
        .await
        .unwrap();
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    assert_eq!(link.answered_version, version + 2);

    links.unlink(ALICE, &link_id).await.unwrap();
    assert!(matches!(
        links.request_sync(ALICE, &link_id).await,
        Err(timada_aliexpress::AliexpressError::NotFound)
    ));
}

#[tokio::test]
async fn an_archived_product_frees_its_item_for_a_new_import() {
    let executor = executor().await;
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let (link_id, product_id) = imported(&executor).await;

    // While the product stands, the item is imported already.
    assert!(matches!(
        links.request_import(ALICE, "1005006123456789").await,
        Err(timada_aliexpress::AliexpressError::AlreadyLinked)
    ));

    // Archived, the link is over: asking again unlinks it and starts anew.
    catalog.archive_product(ALICE, &product_id).await.unwrap();
    let again = links
        .request_import(ALICE, "1005006123456789")
        .await
        .unwrap();
    assert_eq!(again, link_id);
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    assert_eq!(link.status, LinkStatus::Importing);
    assert_eq!(link.product_id, None);
    assert!(link.skus.is_empty());
    assert!(links.start_import("node-a", &link_id).await.unwrap());
}

async fn imported(executor: &evento::Turso) -> (String, String) {
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let link_id = links
        .request_import(ALICE, "1005006123456789")
        .await
        .unwrap();
    let mapped = map_product(&fixture(), "EUR", Some(&PRICING)).unwrap();
    let imported = import_one(
        &catalog,
        ALICE,
        &mapped.seed,
        Path::new(""),
        Arc::new(NoImages),
        &ImportOptions {
            publish: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let skus = track_imported(
        &catalog,
        ALICE,
        &imported.product_id,
        &imported.variant_ids,
        &mapped.skus,
    )
    .await
    .unwrap();
    links
        .record_import(ALICE, &link_id, &imported.product_id, skus)
        .await
        .unwrap();
    (link_id, imported.product_id)
}

fn sku(id: &str, cost: u64, available: u32, properties: &[(&str, &str)]) -> AeSku {
    AeSku {
        id: id.into(),
        cost_cents: Some(cost),
        currency: None,
        available,
        properties: properties
            .iter()
            .map(|(name, value)| AeSkuProperty {
                name: (*name).into(),
                value: (*value).into(),
                image: None,
            })
            .collect(),
        attr: None,
    }
}

#[tokio::test]
async fn a_read_finds_new_combinations_the_merchant_adds_or_ignores() {
    let executor = executor().await;
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let (link_id, product_id) = imported(&executor).await;

    // AliExpress now sells green, and a size the product had not.
    let mut read = fixture();
    read.skus.push(sku(
        "12000037004",
        300,
        5,
        &[("Color", "Green"), ("Size", "S")],
    ));
    read.skus.push(sku(
        "12000037005",
        300,
        6,
        &[("Color", "Rose"), ("Size", "M")],
    ));
    read.skus.push(sku(
        "12000037006",
        300,
        7,
        &[("Color", "Rose"), ("Size", "S"), ("Pack", "2")],
    ));
    let (link, version) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    let found: Vec<_> = outcome
        .found
        .iter()
        .map(|sku| sku.sku_id.as_str())
        .collect();
    // The unpriced grey one is not offered.
    assert_eq!(found, ["12000037004", "12000037005", "12000037006"]);
    links
        .record_sync(
            "sync",
            &link_id,
            version,
            SyncRead {
                skus: outcome.skus,
                offline: false,
                found: Some(outcome.found),
            },
        )
        .await
        .unwrap();
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    assert_eq!(link.found.len(), 3);

    let product = catalog.load_product(&product_id).await.unwrap().unwrap();
    assert!(
        matches!(fit(&product, &link.found[0]), Fit::Fits { ref new_values, .. } if new_values.len() == 1)
    );
    assert!(
        matches!(fit(&product, &link.found[1]), Fit::Fits { ref new_values, .. } if new_values.len() == 1)
    );
    assert_eq!(fit(&product, &link.found[2]), Fit::NewOption("Pack".into()));

    // Ignored, then offered again.
    links
        .ignore_combination(ALICE, &link_id, "12000037005")
        .await
        .unwrap();
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    let offered: Vec<_> = link.offered().map(|sku| sku.sku_id.as_str()).collect();
    assert_eq!(offered, ["12000037004", "12000037006"]);
    assert!(matches!(
        links.ignore_combination(ALICE, &link_id, "nope").await,
        Err(timada_aliexpress::AliexpressError::CombinationNotFound)
    ));

    // Adding all that is offered adds green and leaves the pack.
    let (link, version) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    let added = add_combinations(&catalog, ALICE, &link, &[], Some(&PRICING), &NoImages)
        .await
        .unwrap();
    assert_eq!(added.skus.len(), 1);
    assert_eq!(added.warnings.len(), 1);
    let green = added.skus[0].variant_id.clone().unwrap();
    let product = catalog.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(product.variants.len(), 3);
    assert_eq!(product.variant(&green).unwrap().price.amount_cents, 600);
    assert!(product.options[0].values.contains(&"Green".to_owned()));
    let stock = catalog
        .load_stock(&product_id, Some(&green))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stock.on_hand, 5);

    // An add that was not recorded links the variant it made.
    let again = add_combinations(
        &catalog,
        ALICE,
        &link,
        &["12000037004".to_owned()],
        Some(&PRICING),
        &NoImages,
    )
    .await
    .unwrap();
    assert_eq!(again.skus, added.skus);
    let product = catalog.load_product(&product_id).await.unwrap().unwrap();
    assert_eq!(product.variants.len(), 3);

    links
        .record_combinations_added(ALICE, &link_id, version, added.skus)
        .await
        .unwrap();
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    assert_eq!(link.skus.len(), 3);
    let found: Vec<_> = link.found.iter().map(|sku| sku.sku_id.as_str()).collect();
    assert_eq!(found, ["12000037005", "12000037006"]);

    links
        .restore_combination(ALICE, &link_id, "12000037005")
        .await
        .unwrap();
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    assert_eq!(link.offered().count(), 2);

    // The next read counts green, and no longer finds it.
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert_eq!(outcome.skus.len(), 3);
    assert_eq!(outcome.found, link.found);

    // Offline: nothing is offered.
    read.status = "offline".into();
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING))
        .await
        .unwrap();
    assert!(outcome.found.is_empty());
}
