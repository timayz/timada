//! The quote a shopper is shown is priced from the catalog's read model, a
//! line it cannot sell read again from the products' own streams; the quote
//! a payment is asked at, from the products' own streams. Both go by the
//! same rules, so they agree once the read model has caught up.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use timada_catalog::{DeliveryDays, Sellable};
use timada_core::db::Db;
use timada_orders::{CatalogReadModel, Command, QuoteSource, StoreCatalog};

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

/// The products' and the stocks' own streams, counting what is read of
/// them.
struct Counted<'a> {
    store: StoreCatalog<'a, evento::Turso>,
    products: AtomicUsize,
    stocks: AtomicUsize,
}

impl<'a> Counted<'a> {
    fn new(orders: &'a Orders) -> Self {
        Self {
            store: StoreCatalog(orders),
            products: AtomicUsize::new(0),
            stocks: AtomicUsize::new(0),
        }
    }

    /// How many products and stocks were read since the last call.
    fn reads(&self) -> (usize, usize) {
        (
            self.products.swap(0, Ordering::Relaxed),
            self.stocks.swap(0, Ordering::Relaxed),
        )
    }
}

impl QuoteSource for Counted<'_> {
    async fn sellable(&self, product_id: &str) -> anyhow::Result<Option<Sellable>> {
        self.products.fetch_add(1, Ordering::Relaxed);
        self.store.sellable(product_id).await
    }

    async fn available(
        &self,
        cart_id: &str,
        product_id: &str,
        variant_id: Option<&str>,
    ) -> anyhow::Result<Option<u32>> {
        self.stocks.fetch_add(1, Ordering::Relaxed);
        self.store.available(cart_id, product_id, variant_id).await
    }

    async fn default_delivery_days(&self) -> anyhow::Result<Option<DeliveryDays>> {
        self.store.default_delivery_days().await
    }
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
    let cart_id = orders
        .start_cart(SHOPPER, item(&mug, None, 2))
        .await
        .unwrap();
    let cart = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();

    // Nothing in the read model yet: the lines are read from the store,
    // which the cart read when it took them, rather than shown off sale.
    let read_model = CatalogReadModel::new(&db, TENANT);
    let store = StoreCatalog(&orders);
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert!(shown.unavailable.is_empty());
    assert_eq!(shown.total_cents, 2 * 1250 + 1000);

    catch_up(&executor, &db).await;
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
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
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert_eq!(shown.total_cents, 2 * 1250 + 1000, "a moment behind");
    assert_eq!(
        orders.quote_cart(&cart_id).await.unwrap().total_cents,
        2 * 1500 + 1000
    );
    catch_up(&executor, &db).await;
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert_eq!(shown.total_cents, 2 * 1500 + 1000);
}

#[tokio::test]
async fn the_shown_quote_reads_stock_and_sale_from_the_read_model() {
    let (executor, db, orders, catalog) = store_with_catalog().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let (mat, beige, _blue) = varied_product(&catalog, "Mat").await;
    catalog.set_stock(ALICE, &mug, None, 1).await.unwrap();
    let cart_id = orders
        .start_cart(SHOPPER, item(&mug, None, 1))
        .await
        .unwrap();
    let cart = orders
        .add_to_cart(SHOPPER, &cart_id, item(&mat, Some(&beige), 1))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let read_model = CatalogReadModel::new(&db, TENANT);
    let store = StoreCatalog(&orders);
    assert_eq!(
        orders
            .quote_from(&cart, &read_model, &store)
            .await
            .unwrap()
            .lines
            .len(),
        2
    );

    // The last mug sold elsewhere; the mat taken off sale. Until the read
    // model has it, both still show on sale, the store being asked only
    // about what the read model cannot sell; the payment's exact quote
    // refuses them.
    catalog.set_stock(ALICE, &mug, None, 0).await.unwrap();
    catalog.unpublish_product(ALICE, &mat).await.unwrap();
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert_eq!(shown.lines.len(), 2, "a moment behind");
    assert_eq!(
        orders.quote_cart(&cart_id).await.unwrap().unavailable.len(),
        2
    );
    catch_up(&executor, &db).await;

    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert!(shown.lines.is_empty());
    assert_eq!(shown.unavailable.len(), 2);
    assert_eq!(shown.out_of_stock.len(), 1);
    assert_eq!(shown.out_of_stock[0].product_id, mug);
    assert_eq!(shown, orders.quote_cart(&cart_id).await.unwrap());
}

/// The store is read for a line the read model cannot sell, and for no
/// other: a product put on sale, or stock put back, a moment ago is priced
/// from it until the read model has it (#558).
#[tokio::test]
async fn the_store_is_read_only_for_a_line_the_read_model_cannot_sell() {
    let (executor, db, orders, catalog) = store_with_catalog().await;
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    catalog.set_stock(ALICE, &mug, None, 1).await.unwrap();
    let cart_id = orders
        .start_cart(SHOPPER, item(&mug, None, 1))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let read_model = CatalogReadModel::new(&db, TENANT);
    let store = Counted::new(&orders);

    // A lamp put on sale a moment ago, which the read model has not got:
    // its product and its stock are read from the store, the mug's not.
    let lamp = product(&catalog, "Lamp", 3000, "EUR").await;
    let cart = orders
        .add_to_cart(SHOPPER, &cart_id, item(&lamp, None, 2))
        .await
        .unwrap();
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert!(shown.unavailable.is_empty());
    assert_eq!(shown.total_cents, 1250 + 2 * 3000);
    assert_eq!(store.reads(), (1, 1));

    catch_up(&executor, &db).await;
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert_eq!(shown.lines.len(), 2);
    assert_eq!(store.reads(), (0, 0));

    // The last mug sold, and one put back a moment ago: the read model
    // has none on the shelf, the store has one.
    catalog.set_stock(ALICE, &mug, None, 0).await.unwrap();
    catch_up(&executor, &db).await;
    catalog.set_stock(ALICE, &mug, None, 1).await.unwrap();
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert!(shown.unavailable.is_empty());
    assert_eq!(store.reads(), (1, 1));

    // Off sale in both: its product alone says so.
    catalog.unpublish_product(ALICE, &lamp).await.unwrap();
    catch_up(&executor, &db).await;
    let shown = orders.quote_from(&cart, &read_model, &store).await.unwrap();
    assert_eq!(shown.unavailable.len(), 1);
    assert_eq!(shown.unavailable[0].product_id, lamp);
    assert_eq!(store.reads(), (1, 0));
}
