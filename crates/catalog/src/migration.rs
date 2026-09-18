use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001CatalogProduct;

sqlite_migration!(
    M0001CatalogProduct,
    "catalog",
    "m0001_catalog_product",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE catalog_product (
                id TEXT PRIMARY KEY,
                sku TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                brand_slug TEXT NOT NULL,
                category_path TEXT NOT NULL,
                archived INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE catalog_product"
        ),
        (
            "CREATE INDEX catalog_product_brand ON catalog_product (brand_slug, archived)",
            "DROP INDEX catalog_product_brand"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001CatalogProduct]
}
