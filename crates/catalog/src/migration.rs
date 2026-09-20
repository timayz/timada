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

pub struct M0003Listing;

sqlite_migration!(
    M0003Listing,
    "catalog",
    "m0003_listing",
    vec_box![M0002Categories],
    vec_box![
        // What the storefront lists, filters and sorts: one row per product,
        // with what pricing, inventory and review say about it.
        (
            "CREATE TABLE catalog_listing (
                product_id TEXT PRIMARY KEY,
                sku TEXT NOT NULL,
                name TEXT NOT NULL,
                brand_name TEXT NOT NULL,
                brand_slug TEXT NOT NULL,
                category_id TEXT,
                category_trail TEXT NOT NULL DEFAULT '/',
                short_description TEXT NOT NULL DEFAULT '',
                thumbnail_url TEXT,
                thumbnail_alt TEXT,
                price_minor INTEGER,
                currency TEXT,
                available INTEGER NOT NULL DEFAULT 0,
                rating_avg REAL,
                review_count INTEGER NOT NULL DEFAULT 0,
                archived INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            "DROP TABLE catalog_listing"
        ),
        (
            "CREATE INDEX catalog_listing_brand ON catalog_listing (brand_slug)",
            "DROP INDEX catalog_listing_brand"
        ),
        (
            "CREATE INDEX catalog_listing_price ON catalog_listing (price_minor)",
            "DROP INDEX catalog_listing_price"
        ),
        // The published reviews the listing's ratings are averaged from.
        (
            "CREATE TABLE catalog_listing_review (
                review_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                rating INTEGER NOT NULL
            )",
            "DROP TABLE catalog_listing_review"
        ),
        (
            "CREATE INDEX catalog_listing_review_product ON catalog_listing_review (product_id)",
            "DROP INDEX catalog_listing_review_product"
        ),
        // Full-text index, its rowid being the listing row's. Accents are
        // folded (`écran` = `ecran`), prefixes of 2 and 3 letters are indexed
        // for search-as-you-type.
        (
            "CREATE VIRTUAL TABLE catalog_listing_fts USING fts5(
                name, brand, category, sku, features,
                tokenize = 'unicode61 remove_diacritics 2',
                prefix = '2 3'
            )",
            "DROP TABLE catalog_listing_fts"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001CatalogProduct, M0002Categories, M0003Listing]
}
