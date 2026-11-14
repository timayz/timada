use std::path::Path;
use std::sync::Arc;

use evento::ProjectionAggregate;
use timada_aliexpress::aggregator::NewSku;
use timada_aliexpress::{
    AeProduct, AeSku, AeSkuProperty, Fit, LinkState, LinkStatus, SyncRead, add_combinations,
    apply_sync, fit, map_product, track_imported,
};
use timada_catalog::{CreateProduct, DefineVariant, OptionSelection};
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
    assert_eq!(stock.on_hand, 20, "120 is plenty, counted as 20");
    catalog.publish_product(ALICE, &product.id).await.unwrap();

    // AliExpress sells fewer, dearer.
    let mut read = fixture();
    read.skus[0].available = 7;
    read.skus[0].cost_cents = Some(500);
    let (link, version) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    assert_eq!(link.status, LinkStatus::Linked);
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
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
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
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
    use timada_aliexpress::{RenewedTokens, TOKEN_CLAIM_STALE_SECS, TokenRenewal};

    let executor = executor().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claim = |actor: &'static str, company: &'static str, expires_at: u64, at: u64| {
        let executor = executor.clone();
        async move {
            timada_aliexpress::claim_token_refresh(&executor, actor, company, expires_at, at)
                .await
                .unwrap()
        }
    };
    let tokens = |access: u8| RenewedTokens {
        access_token_sealed: vec![access],
        refresh_token_sealed: vec![access, 1],
        expires_at: now + 86_400,
        refresh_expires_at: 0,
        account: "buyer".to_owned(),
    };
    let claimed = TokenRenewal::Claimed { standing: None };
    assert_eq!(
        claim("node-a", "company-1", 1_700_000_000, now).await,
        claimed
    );
    assert_eq!(
        claim("node-b", "company-1", 1_700_000_000, now).await,
        TokenRenewal::Held
    );
    // The next token is a renewal of its own, and so is another company's.
    assert_eq!(
        claim("node-b", "company-1", 1_700_086_400, now).await,
        claimed
    );
    assert_eq!(
        claim("node-b", "company-2", 1_700_000_000, now).await,
        claimed
    );

    // Node a went quiet before recording anything: b takes over once the
    // claim is stale, and a, answered late, records nothing over b's. The
    // claim dates from its event, which may come a second or more after
    // `now`: read that time off the stream rather than reckon it from `now`.
    let claimed_at = timada_aliexpress::load_token_refresh(&executor, "company-1", 1_700_000_000)
        .await
        .unwrap()
        .unwrap()
        .claimed_at;
    let stale = claimed_at + TOKEN_CLAIM_STALE_SECS + 1;
    assert_eq!(
        claim("node-b", "company-1", 1_700_000_000, stale).await,
        claimed
    );
    for (actor, access) in [("node-b", 2), ("node-a", 1)] {
        timada_aliexpress::record_token_renewal(
            &executor,
            actor,
            "company-1",
            1_700_000_000,
            &tokens(access),
        )
        .await
        .unwrap();
    }
    // Whoever finds the access due next applies b's tokens, asking nothing.
    assert_eq!(
        claim("node-c", "company-1", 1_700_000_000, stale + 3_600).await,
        TokenRenewal::Renewed(tokens(2))
    );
}

/// AliExpress answers with tokens lapsing when the access renewed does
/// (#560): they are the company's while the claim holds, then the claim is
/// taken over to ask again with them, and the first answer that renews the
/// access stands over any that follows.
#[tokio::test]
async fn tokens_that_renew_nothing_give_way_once_the_claim_is_stale() {
    use timada_aliexpress::{RenewedTokens, TOKEN_CLAIM_STALE_SECS, TokenRenewal};

    let executor = executor().await;
    let renewing = 1_700_000_000;
    let tokens = |access: u8, expires_at: u64| RenewedTokens {
        access_token_sealed: vec![access],
        refresh_token_sealed: vec![access, 1],
        expires_at,
        refresh_expires_at: 0,
        account: "buyer".to_owned(),
    };
    let unmoved = tokens(1, renewing);
    let renewed = tokens(2, renewing + 86_400);
    let claim = |at: u64| {
        let executor = executor.clone();
        async move {
            timada_aliexpress::claim_token_refresh(&executor, "node-a", "company-1", renewing, at)
                .await
                .unwrap()
        }
    };
    let record = |tokens: RenewedTokens| {
        let executor = executor.clone();
        async move {
            timada_aliexpress::record_token_renewal(
                &executor,
                "node-a",
                "company-1",
                renewing,
                &tokens,
            )
            .await
            .unwrap()
        }
    };

    assert!(!unmoved.renews(renewing));
    assert!(renewed.renews(renewing));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert_eq!(claim(now).await, TokenRenewal::Claimed { standing: None });
    record(unmoved.clone()).await;
    // Recorded once: the same answer again writes nothing.
    record(unmoved.clone()).await;
    let state = timada_aliexpress::load_token_refresh(&executor, "company-1", renewing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.renewed.as_ref(), Some(&unmoved));
    assert_eq!(state.aggregate_version().unwrap(), 2);
    // Meanwhile every look gets them, asking nothing.
    assert_eq!(claim(now + 1).await, TokenRenewal::Renewed(unmoved.clone()));
    // Stale, the claim is taken again, standing on what AliExpress gave.
    let stale = state.claimed_at + TOKEN_CLAIM_STALE_SECS;
    assert_eq!(
        claim(stale).await,
        TokenRenewal::Claimed {
            standing: Some(unmoved.clone())
        }
    );
    record(renewed.clone()).await;
    // A late answer that renews nothing changes nothing any more.
    record(tokens(3, renewing)).await;
    assert_eq!(
        claim(stale + 3_600).await,
        TokenRenewal::Renewed(renewed.clone())
    );
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
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
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
    let outcome = apply_sync(&catalog, "sync", &link, Some(&fixture()), Some(&PRICING), 0)
        .await
        .unwrap();
    assert!(!outcome.differs_from(&link));

    // AliExpress sold some: the catalog counts them, the link holds no count.
    let mut read = fixture();
    read.skus[0].available = 7;
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
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
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
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
    let outcome = apply_sync(&catalog, "sync", &found, Some(&read), Some(&PRICING), 0)
        .await
        .unwrap();
    assert!(!outcome.differs_from(&found));
    let mut read = fixture();
    read.skus
        .push(sku("12000037009", 800, 1, &[("Color", "Green")]));
    let outcome = apply_sync(&catalog, "sync", &found, Some(&read), Some(&PRICING), 0)
        .await
        .unwrap();
    assert!(
        outcome.found_differs_from(&found),
        "it costs something else"
    );

    // The cost moved: the link holds what it cost.
    let mut read = fixture();
    read.skus[0].cost_cents = Some(500);
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
        .await
        .unwrap();
    assert!(outcome.differs_from(&link));

    // AliExpress stopped selling it.
    let mut read = fixture();
    read.status = "offline".into();
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
        .await
        .unwrap();
    assert!(outcome.differs_from(&link));
}

#[tokio::test]
async fn a_price_stays_while_the_rule_is_within_the_tolerance() {
    let executor = executor().await;
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let (link_id, product_id) = imported(&executor).await;
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    let rose = catalog
        .load_product(&product_id)
        .await
        .unwrap()
        .unwrap()
        .variants[0]
        .id
        .clone();
    let price = async || {
        let product = catalog.load_product(&product_id).await.unwrap().unwrap();
        product.variant(&rose).unwrap().price.amount_cents
    };
    // The rule gives twice the cost: 4.31 is 8.62.
    assert_eq!(price().await, 862);
    let read_at = |cost: u64| {
        let mut read = fixture();
        read.skus[0].cost_cents = Some(cost);
        read
    };
    // 2 % of 8.62 is 17 cents.

    // 8.70 is 8 cents off, up: the price stays, the cost is still recorded.
    let outcome = apply_sync(
        &catalog,
        "sync",
        &link,
        Some(&read_at(435)),
        Some(&PRICING),
        200,
    )
    .await
    .unwrap();
    assert_eq!(outcome.repriced, 0);
    assert_eq!(outcome.skus[0].cost_cents, 435);
    assert!(outcome.differs_from(&link));
    assert_eq!(price().await, 862);

    // 8.46 is 16 cents off, down: it stays too.
    let outcome = apply_sync(
        &catalog,
        "sync",
        &link,
        Some(&read_at(423)),
        Some(&PRICING),
        200,
    )
    .await
    .unwrap();
    assert_eq!(outcome.repriced, 0);
    assert_eq!(price().await, 862);

    // 8.80 is 18 cents off: past it, the price follows the rule.
    let outcome = apply_sync(
        &catalog,
        "sync",
        &link,
        Some(&read_at(440)),
        Some(&PRICING),
        200,
    )
    .await
    .unwrap();
    assert_eq!(outcome.repriced, 1);
    assert_eq!(price().await, 880);

    // Measured from the price as it stands, small moves that add up do not
    // slip through one by one: from 8.80, 8.97 is within it, 8.98 is not.
    let outcome = apply_sync(
        &catalog,
        "sync",
        &link,
        Some(&read_at(449)),
        Some(&PRICING),
        200,
    )
    .await
    .unwrap();
    assert_eq!(outcome.repriced, 1);
    assert_eq!(price().await, 898);

    // Without a tolerance, every move follows.
    let outcome = apply_sync(
        &catalog,
        "sync",
        &link,
        Some(&read_at(450)),
        Some(&PRICING),
        0,
    )
    .await
    .unwrap();
    assert_eq!(outcome.repriced, 1);
    assert_eq!(price().await, 900);
}

/// A read is recorded against the version of the link it started from: one
/// recorded in between wins, and nothing but reads and the merchant's own
/// changes ever goes on the stream (#472).
#[tokio::test]
async fn a_read_records_against_the_version_it_started_from() {
    let executor = executor().await;
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let (link_id, _) = imported(&executor).await;

    let (_, version) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    links
        .record_sync(ALICE, &link_id, version, SyncRead::default())
        .await
        .unwrap();
    assert!(matches!(
        links
            .record_sync("sync", &link_id, version, SyncRead::default())
            .await,
        Err(timada_aliexpress::AliexpressError::Write(
            evento::WriteError::InvalidOriginalVersion
        ))
    ));
    let (link, after) = links.load_link_versioned(&link_id).await.unwrap().unwrap();
    assert_eq!(after, version + 1);
    assert!(link.last_synced_at.is_some());
    links
        .record_sync_failure(ALICE, &link_id, "no")
        .await
        .unwrap();
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    assert_eq!(link.last_error.as_deref(), Some("no"));
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
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
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
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
        .await
        .unwrap();
    assert_eq!(outcome.skus.len(), 3);
    assert_eq!(outcome.found, link.found);

    // Offline: nothing is offered.
    read.status = "offline".into();
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(&PRICING), 0)
        .await
        .unwrap();
    assert!(outcome.found.is_empty());
}

#[tokio::test]
async fn a_combination_a_hidden_option_leaves_off_sale_is_not_offered_as_fitting() {
    let executor = executor().await;
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let product_id = catalog
        .create_product(
            ALICE,
            CreateProduct {
                name: "Cat Tunnel".into(),
                slug: None,
            },
        )
        .await
        .unwrap()
        .id;
    let from = catalog
        .define_option(
            ALICE,
            &product_id,
            "Ships From",
            vec!["China".into(), "France".into()],
        )
        .await
        .unwrap();
    let colour = catalog
        .define_option(ALICE, &product_id, "Color", vec!["Rose".into()])
        .await
        .unwrap();
    let mut french = None;
    for warehouse in ["China", "France"] {
        let variant = catalog
            .define_variant(
                ALICE,
                &product_id,
                DefineVariant {
                    selections: vec![
                        OptionSelection {
                            option_id: from.clone(),
                            value: warehouse.into(),
                        },
                        OptionSelection {
                            option_id: colour.clone(),
                            value: "Rose".into(),
                        },
                    ],
                    sku: None,
                    amount_cents: 1000,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap();
        if warehouse == "France" {
            french = Some(variant);
        }
    }
    catalog
        .hide_option(ALICE, &product_id, &from, "France")
        .await
        .unwrap();
    let product = catalog.load_product(&product_id).await.unwrap().unwrap();
    let found = |warehouse: &str, shade: &str| NewSku {
        sku_id: "12000037009".into(),
        selections: vec![
            ("Ships From".into(), warehouse.into()),
            ("Color".into(), shade.into()),
        ],
        cost_cents: 300,
        available: 5,
        image: None,
    };

    // Another warehouse would not be on sale; the merchant is told why.
    let fitted = fit(&product, &found("China", "Blue"));
    assert_eq!(
        fitted,
        Fit::OffSale {
            option: "Ships From".into(),
            value: "France".into(),
        }
    );
    assert_eq!(
        fitted.reason().as_deref(),
        Some("Ships From is hidden from shoppers at France: it would not be on sale.")
    );

    // The value on sale fits, and what is a variant already stays linkable,
    // on sale or not.
    assert!(matches!(
        fit(&product, &found("France", "Blue")),
        Fit::Fits { ref new_values, .. } if new_values.len() == 1
    ));
    assert_eq!(
        fit(&product, &found("France", "Rose")),
        Fit::Exists(french.unwrap())
    );
    assert!(matches!(
        fit(&product, &found("China", "Rose")),
        Fit::Exists(_)
    ));
}

#[tokio::test]
async fn a_read_brings_in_the_days_delivery_takes() {
    let executor = executor().await;
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let links = timada_aliexpress::Command::new(executor.clone(), TENANT);
    let link_id = links
        .request_import(ALICE, "1005006123456789")
        .await
        .unwrap();
    assert!(links.start_import("node-a", &link_id).await.unwrap());
    let mapped = map_product(&fixture(), "EUR", Some(&PRICING)).unwrap();
    let imported = import_one(
        &catalog,
        ALICE,
        &mapped.seed,
        Path::new(""),
        Arc::new(NoImages),
        &ImportOptions::default(),
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
    let link = links.load_link_by_id(&link_id).await.unwrap().unwrap();
    let days = |min, max| timada_catalog::DeliveryDays::new(min, max).unwrap();
    let product = async || {
        catalog
            .load_product(&imported.product_id)
            .await
            .unwrap()
            .unwrap()
    };
    let sync = async |read: &AeProduct| {
        apply_sync(&catalog, "sync", &link, Some(read), Some(&PRICING), 0)
            .await
            .unwrap()
    };

    // AliExpress's days replace the merchant's.
    catalog
        .set_product_delivery_days(ALICE, &imported.product_id, Some(days(2, 3)))
        .await
        .unwrap();
    let mut read = fixture();
    read.delivery_days = Some(days(8, 15));
    sync(&read).await;
    let synced = product().await;
    assert_eq!(synced.delivery_days, Some(days(8, 15)));

    // The same days write nothing.
    sync(&read).await;
    assert_eq!(
        product().await.aggregate_version().unwrap(),
        synced.aggregate_version().unwrap()
    );

    // An item AliExpress no longer sells keeps the last days it said.
    let mut withdrawn = fixture();
    withdrawn.status = "offline".into();
    sync(&withdrawn).await;
    assert_eq!(product().await.delivery_days, Some(days(8, 15)));

    // No days given: the shop's default stands in.
    sync(&fixture()).await;
    assert_eq!(product().await.delivery_days, None);
}
