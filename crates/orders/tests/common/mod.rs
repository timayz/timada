#![allow(dead_code)]

use evento::migrator::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx_migrator::Info;
use timada_catalog::{CreateProduct, DefineVariant, OptionSelection};
use timada_orders::{CartItem, Checkout, Command, STOREFRONT_ACTOR, ShippingAddress};

pub type Orders = Command<evento::Sqlite>;
pub type Catalog = timada_catalog::Command<evento::Sqlite>;

/// A fresh in-memory event store with the read-model schemas applied.
pub async fn executor_with_pool() -> (evento::Sqlite, sqlx::SqlitePool) {
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
        .add_migrations(timada_orders::migrations())
        .unwrap();
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .unwrap();
    drop(conn);
    (pool.clone().into(), pool)
}

/// The two gateways over one store.
pub async fn store() -> (Orders, Catalog) {
    let (executor, _) = executor_with_pool().await;
    (Command(executor.clone()), timada_catalog::Command(executor))
}

pub const ALICE: &str = "user-alice";
pub const SHOPPER: &str = STOREFRONT_ACTOR;

/// A published product at `amount_cents` of `currency`.
pub async fn product(catalog: &Catalog, name: &str, amount_cents: u64, currency: &str) -> String {
    let product = catalog
        .create_product(
            ALICE,
            CreateProduct {
                name: name.to_owned(),
                slug: None,
            },
        )
        .await
        .unwrap();
    catalog
        .price_product(ALICE, &product.id, amount_cents, currency)
        .await
        .unwrap();
    catalog.publish_product(ALICE, &product.id).await.unwrap();
    product.id
}

/// A published product sold in two colours, `Beige` at 10.00 EUR (code
/// `MAT-B`) and `Blue` at 12.00 EUR; returns the product and both variants.
pub async fn varied_product(catalog: &Catalog, name: &str) -> (String, String, String) {
    let product = catalog
        .create_product(
            ALICE,
            CreateProduct {
                name: name.to_owned(),
                slug: None,
            },
        )
        .await
        .unwrap();
    let colour = catalog
        .define_option(
            ALICE,
            &product.id,
            "Colour",
            vec!["Beige".into(), "Blue".into()],
        )
        .await
        .unwrap();
    let variant = |value: &str, amount_cents: u64, sku: Option<&str>| DefineVariant {
        selections: vec![OptionSelection {
            option_id: colour.clone(),
            value: value.to_owned(),
        }],
        sku: sku.map(str::to_owned),
        amount_cents,
        currency: "EUR".into(),
        image_id: None,
    };
    let beige = catalog
        .define_variant(ALICE, &product.id, variant("Beige", 1000, Some("MAT-B")))
        .await
        .unwrap();
    let blue = catalog
        .define_variant(ALICE, &product.id, variant("Blue", 1200, None))
        .await
        .unwrap();
    catalog.publish_product(ALICE, &product.id).await.unwrap();
    (product.id, beige, blue)
}

pub fn item(product_id: &str, variant_id: Option<&str>, quantity: u32) -> CartItem {
    CartItem {
        product_id: product_id.to_owned(),
        variant_id: variant_id.map(str::to_owned),
        quantity,
    }
}

pub fn checkout() -> Checkout {
    Checkout {
        email: "ada@example.org".into(),
        shipping: ShippingAddress {
            name: "Ada Lovelace".into(),
            line1: "1 rue de la Paix".into(),
            line2: None,
            postal_code: "75002".into(),
            city: "Paris".into(),
            country: "FR".into(),
            phone: None,
        },
        note: None,
        expected_total_cents: None,
    }
}

/// A cart holding `quantity` of the product.
pub async fn cart_with(orders: &Orders, product_id: &str, quantity: u32) -> String {
    let cart_id = orders.create_cart(SHOPPER).await.unwrap();
    orders
        .add_to_cart(SHOPPER, &cart_id, item(product_id, None, quantity))
        .await
        .unwrap();
    cart_id
}
