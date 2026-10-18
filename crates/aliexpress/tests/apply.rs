use std::path::Path;
use std::sync::Arc;

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_aliexpress::{AeProduct, LinkStatus, apply_sync, map_product, track_imported};
use timada_catalog_import::{ImportOptions, NoImages, import_one};
use timada_tenancy::{PricingRule, Rounding};

const ALICE: &str = "user-alice";

async fn executor() -> evento::Sqlite {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
    migrator
        .add_migrations(timada_catalog::migrations())
        .unwrap();
    migrator
        .add_migrations(timada_aliexpress::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    pool.into()
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

#[tokio::test]
async fn an_import_makes_a_counted_draft_that_a_read_brings_in_step() {
    let executor = executor().await;
    let catalog = timada_catalog::Command(executor.clone());
    let links = timada_aliexpress::Command(executor.clone());

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

    let mapped = map_product(&fixture(), "EUR", Some(RULE)).unwrap();
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
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(RULE))
        .await
        .unwrap();
    assert!(!outcome.offline);
    assert_eq!(outcome.repriced, 1);
    links
        .record_sync("sync", &link_id, version, outcome.skus, false)
        .await
        .unwrap();
    // A read that started from the same version lost the race.
    assert!(
        links
            .record_sync("sync", &link_id, version, Vec::new(), false)
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
    let outcome = apply_sync(&catalog, "sync", &link, Some(&read), Some(RULE))
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
    let links = timada_aliexpress::Command(executor);
    assert!(
        links
            .claim_token_refresh("node-a", 1_700_000_000)
            .await
            .unwrap()
    );
    assert!(
        !links
            .claim_token_refresh("node-b", 1_700_000_000)
            .await
            .unwrap()
    );
    // The next token is a renewal of its own.
    assert!(
        links
            .claim_token_refresh("node-b", 1_700_086_400)
            .await
            .unwrap()
    );
}
