mod common;

use common::*;
use timada_catalog::{CatalogError, DefineVariant, OptionSelection, StockLine, StockSummary};

const HOUR: u64 = 3600;

fn line(product_id: &str, variant_id: Option<&str>, quantity: u32) -> StockLine {
    StockLine {
        product_id: product_id.to_owned(),
        variant_id: variant_id.map(str::to_owned),
        quantity,
    }
}

/// A product with two sizes, with its variant ids.
async fn sized(cmd: &timada_catalog::Command<evento::Sqlite>) -> (String, String, String) {
    let product = create_product(cmd, "Cat Tree").await;
    let size = cmd
        .define_option(ALICE, &product.id, "Size", vec!["S".into(), "L".into()])
        .await
        .unwrap();
    let mut ids = Vec::new();
    for value in ["S", "L"] {
        ids.push(
            cmd.define_variant(
                ALICE,
                &product.id,
                DefineVariant {
                    selections: vec![OptionSelection {
                        option_id: size.clone(),
                        value: value.into(),
                    }],
                    sku: None,
                    amount_cents: 1000,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await
            .unwrap(),
        );
    }
    (product.id, ids[0].clone(), ids[1].clone())
}

#[tokio::test]
async fn an_untracked_thing_sells_without_limit() {
    let cmd = command().await;
    let product = create_product(&cmd, "Mug").await;
    assert!(cmd.load_stock(&product.id, None).await.unwrap().is_none());
    cmd.reserve_stock(ALICE, "cart-1", &[line(&product.id, None, 500)], HOUR)
        .await
        .unwrap();
    assert!(cmd.load_stock(&product.id, None).await.unwrap().is_none());
}

#[tokio::test]
async fn setting_a_stock_tracks_it_and_counts_it_again() {
    let cmd = command().await;
    let product = create_product(&cmd, "Mug").await;
    cmd.set_stock(ALICE, &product.id, None, 3).await.unwrap();
    let stock = cmd.load_stock(&product.id, None).await.unwrap().unwrap();
    assert!(stock.tracked);
    assert_eq!(stock.on_hand, 3);
    cmd.set_stock(ALICE, &product.id, None, 7).await.unwrap();
    let stock = cmd.load_stock(&product.id, None).await.unwrap().unwrap();
    assert_eq!(stock.on_hand, 7);
    assert_eq!(stock.available_at(0), Some(7));

    cmd.untrack_stock(ALICE, &product.id, None).await.unwrap();
    let stock = cmd.load_stock(&product.id, None).await.unwrap().unwrap();
    assert!(!stock.tracked);
    assert_eq!(stock.available_at(0), None);
}

#[tokio::test]
async fn a_stock_names_a_variant_exactly_when_the_product_sells_variants() {
    let cmd = command().await;
    let (product_id, small, _) = sized(&cmd).await;
    assert!(matches!(
        cmd.set_stock(ALICE, &product_id, None, 1).await,
        Err(CatalogError::VariantNotFound)
    ));
    assert!(matches!(
        cmd.set_stock(ALICE, &product_id, Some("nope"), 1).await,
        Err(CatalogError::VariantNotFound)
    ));
    assert!(matches!(
        cmd.set_stock(ALICE, &product_id, Some(&small), 1_000_001)
            .await,
        Err(CatalogError::InvalidStock)
    ));
    cmd.set_stock(ALICE, &product_id, Some(&small), 2)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_checkout_cannot_hold_more_than_is_left() {
    let cmd = command().await;
    let (product_id, small, large) = sized(&cmd).await;
    cmd.set_stock(ALICE, &product_id, Some(&small), 2)
        .await
        .unwrap();
    cmd.set_stock(ALICE, &product_id, Some(&large), 1)
        .await
        .unwrap();

    // Two lines of one variant (an offer split it) are held as one.
    cmd.reserve_stock(
        ALICE,
        "cart-1",
        &[
            line(&product_id, Some(&small), 1),
            line(&product_id, Some(&small), 1),
        ],
        HOUR,
    )
    .await
    .unwrap();
    let err = cmd
        .reserve_stock(
            ALICE,
            "cart-2",
            &[
                line(&product_id, Some(&large), 1),
                line(&product_id, Some(&small), 1),
            ],
            HOUR,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        CatalogError::OutOfStock { available: 0, ref variant_id, .. } if variant_id.as_deref() == Some(small.as_str())
    ));
    // Nothing of the refused checkout stays held.
    let large_stock = cmd
        .load_stock(&product_id, Some(&large))
        .await
        .unwrap()
        .unwrap();
    assert!(large_stock.holds.is_empty());

    // The first checkout may ask again: its own hold does not count.
    cmd.reserve_stock(ALICE, "cart-1", &[line(&product_id, Some(&small), 2)], HOUR)
        .await
        .unwrap();
    cmd.release_stock(ALICE, "cart-1", &[line(&product_id, Some(&small), 2)])
        .await
        .unwrap();
    cmd.reserve_stock(ALICE, "cart-2", &[line(&product_id, Some(&small), 2)], HOUR)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_lapsed_hold_no_longer_counts() {
    let cmd = command().await;
    let product = create_product(&cmd, "Mug").await;
    cmd.set_stock(ALICE, &product.id, None, 1).await.unwrap();
    cmd.reserve_stock(ALICE, "cart-1", &[line(&product.id, None, 1)], 0)
        .await
        .unwrap();
    cmd.reserve_stock(ALICE, "cart-2", &[line(&product.id, None, 1)], HOUR)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_order_takes_its_share_once_and_gives_it_back_once() {
    let cmd = command().await;
    let product = create_product(&cmd, "Mug").await;
    cmd.set_stock(ALICE, &product.id, None, 5).await.unwrap();
    let lines = [line(&product.id, None, 2)];
    cmd.reserve_stock(ALICE, "cart-1", &lines, HOUR)
        .await
        .unwrap();
    cmd.consume_stock(ALICE, "cart-1", "order-1", &lines)
        .await
        .unwrap();
    cmd.consume_stock(ALICE, "cart-1", "order-1", &lines)
        .await
        .unwrap();
    let stock = cmd.load_stock(&product.id, None).await.unwrap().unwrap();
    assert_eq!(stock.on_hand, 3);
    assert!(stock.holds.is_empty());
    // A checkout of a cart whose order took its share holds nothing more.
    cmd.reserve_stock(ALICE, "cart-1", &lines, HOUR)
        .await
        .unwrap();
    let stock = cmd.load_stock(&product.id, None).await.unwrap().unwrap();
    assert!(stock.holds.is_empty());

    cmd.restore_stock(ALICE, "order-1", &lines).await.unwrap();
    cmd.restore_stock(ALICE, "order-1", &lines).await.unwrap();
    let stock = cmd.load_stock(&product.id, None).await.unwrap().unwrap();
    assert_eq!(stock.on_hand, 5);
}

#[tokio::test]
async fn the_read_model_sums_a_product_up() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = timada_catalog::Command::new(executor.clone(), TENANT);
    let (product_id, small, large) = sized(&cmd).await;
    cmd.set_stock(ALICE, &product_id, Some(&small), 0)
        .await
        .unwrap();
    let product = cmd.load_product(&product_id).await.unwrap().unwrap();
    timada_catalog::upsert_product(&pool, "t1", &product)
        .await
        .unwrap();
    let state = async |pool: sqlx::SqlitePool| {
        timada_catalog::product_by_id(&pool, "t1", &product_id)
            .await
            .unwrap()
            .unwrap()
            .stock_state
    };
    let upsert = async |variant: &str| {
        let stock = cmd
            .load_stock(&product.id, Some(variant))
            .await
            .unwrap()
            .unwrap();
        timada_catalog::upsert_stock(&pool, "t1", &stock, Some(&product))
            .await
            .unwrap();
    };
    upsert(&small).await;
    // The large one is not counted: something is still to be had.
    assert_eq!(state(pool.clone()).await, "in_stock");
    cmd.set_stock(ALICE, &product.id, Some(&large), 0)
        .await
        .unwrap();
    upsert(&large).await;
    assert_eq!(state(pool.clone()).await, StockSummary::SoldOut.as_str());
}
