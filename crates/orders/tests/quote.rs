//! The quote a shopper is shown is priced from the catalog's read model;
//! the quote a payment is asked at, from the products' own streams. Both go
//! by the same rules, so they agree once the read model has caught up.

mod common;

use common::*;
use timada_catalog::DeliveryDays;
use timada_core::db::Db;
use timada_orders::{CatalogReadModel, Command};

/// A store with the catalog's read model in it too, and the gateways.
async fn store_with_catalog() -> (evento::Turso, Db, Orders, Catalog) {
    let (executor, db) = executor_with_pool().await;
    let mut conn = executor.connect().unwrap();
    timada_catalog::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (
        executor.clone(),
        db,
        Command::new(executor.clone(), TENANT),
        timada_catalog::Command::new(executor, TENANT),
    )
}

async fn catch_up(executor: &evento::Turso, db: &Db) {
    timada_catalog::read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_shown_quote_is_priced_from_the_read_model_and_agrees_with_the_exact_one() {
    let (executor, db, orders, catalog) = store_with_catalog().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let (mat, beige, _blue) = varied_product(&catalog, "Mat").await;
    catalog
        .set_default_delivery_days(ALICE, Some(DeliveryDays { min: 2, max: 4 }))
        .await
        .unwrap();
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 2))
        .await
        .unwrap();
    let cart = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();

    // Nothing in the read model yet: the lines cannot be priced from it.
    let read_model = CatalogReadModel::new(&db, TENANT);
    let shown = orders.quote_from(&cart, &read_model).await.unwrap();
    assert!(shown.lines.is_empty());
    assert_eq!(shown.unavailable.len(), 2);

    catch_up(&executor, &db).await;
    let shown = orders.quote_from(&cart, &read_model).await.unwrap();
    let exact = orders.quote_cart(&cart_id).await.unwrap();
    assert_eq!(shown, exact);
    assert_eq!(shown.total_cents, 2 * 1250 + 1000);
    assert_eq!(shown.lines[1].sku.as_deref(), Some("MAT-B"));
    assert_eq!(shown.lines[1].variant_label.as_deref(), Some("Beige"));
    assert_eq!(shown.lines[0].tax_category, "standard");
    assert_eq!(
        shown.delivery_window(),
        Some(DeliveryDays { min: 2, max: 4 }),
        "the shop's default, from the read model"
    );

    // A price that moved shows once the read model caught up; the exact
    // quote has it at once, and the payment refuses the stale total.
    catalog
        .price_product(ALICE, &mug, 1500, "EUR")
        .await
        .unwrap();
    let shown = orders.quote_from(&cart, &read_model).await.unwrap();
    assert_eq!(shown.total_cents, 2 * 1250 + 1000, "a moment behind");
    assert_eq!(
        orders.quote_cart(&cart_id).await.unwrap().total_cents,
        2 * 1500 + 1000
    );
    catch_up(&executor, &db).await;
    let shown = orders.quote_from(&cart, &read_model).await.unwrap();
    assert_eq!(shown.total_cents, 2 * 1500 + 1000);
}

#[tokio::test]
async fn the_shown_quote_reads_stock_and_sale_from_the_read_model() {
    let (executor, db, orders, catalog) = store_with_catalog().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let (mat, beige, _blue) = varied_product(&catalog, "Mat").await;
    catalog.set_stock(ALICE, &mug, None, 1).await.unwrap();
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();
    orders
        .add_to_cart(SHOPPER, &cart_id, item(&mug, None, 1))
        .await
        .unwrap();
    let cart = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let read_model = CatalogReadModel::new(&db, TENANT);
    assert_eq!(
        orders
            .quote_from(&cart, &read_model)
            .await
            .unwrap()
            .lines
            .len(),
        2
    );

    // The last mug sold elsewhere; the mat taken off sale.
    catalog.set_stock(ALICE, &mug, None, 0).await.unwrap();
    catalog.unpublish_product(ALICE, &mat).await.unwrap();
    catch_up(&executor, &db).await;

    let shown = orders.quote_from(&cart, &read_model).await.unwrap();
    assert!(shown.lines.is_empty());
    assert_eq!(shown.unavailable.len(), 2);
    assert_eq!(shown.out_of_stock.len(), 1);
    assert_eq!(shown.out_of_stock[0].product_id, mug);
    assert_eq!(shown, orders.quote_cart(&cart_id).await.unwrap());
}
