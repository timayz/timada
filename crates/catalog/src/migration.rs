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

pub struct M0002Categories;

sqlite_migration!(
    M0002Categories,
    "catalog",
    "m0002_categories",
    vec_box![M0001CatalogProduct],
    vec_box![
        (
            "CREATE TABLE catalog_category (
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                parent_id TEXT,
                position INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE catalog_category"
        ),
        (
            "CREATE INDEX catalog_category_parent ON catalog_category (parent_id, position, name)",
            "DROP INDEX catalog_category_parent"
        ),
        (
            "ALTER TABLE catalog_product ADD COLUMN category_id TEXT",
            "ALTER TABLE catalog_product DROP COLUMN category_id"
        ),
        (
            "CREATE INDEX catalog_product_category ON catalog_product (category_id, archived)",
            "DROP INDEX catalog_product_category"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001CatalogProduct, M0002Categories]
}
