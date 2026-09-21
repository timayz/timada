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

pub struct M0004SpecFacets;

sqlite_migration!(
    M0004SpecFacets,
    "catalog",
    "m0004_spec_facets",
    vec_box![M0003Listing],
    vec_box![
        (
            "ALTER TABLE catalog_category ADD COLUMN facets TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE catalog_category DROP COLUMN facets"
        ),
        // The technical sheet of the listed products, a line per row: what
        // spec filters match and count.
        (
            "CREATE TABLE catalog_listing_spec (
                product_id TEXT NOT NULL,
                spec_group TEXT NOT NULL,
                label TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (product_id, spec_group, label)
            )",
            "DROP TABLE catalog_listing_spec"
        ),
        (
            "CREATE INDEX catalog_listing_spec_value
             ON catalog_listing_spec (spec_group, label, value)",
            "DROP INDEX catalog_listing_spec_value"
        )
    ]
);

pub struct M0005ListingSortName;

sqlite_migration!(
    M0005ListingSortName,
    "catalog",
    "m0005_listing_sort_name",
    vec_box![M0004SpecFacets],
    vec_box![(
        // The name as it is sorted: lower case, accents folded.
        "ALTER TABLE catalog_listing ADD COLUMN sort_name TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE catalog_listing DROP COLUMN sort_name"
    )]
);

pub struct M0006ListingPrices;

sqlite_migration!(
    M0006ListingPrices,
    "catalog",
    "m0006_listing_prices",
    vec_box![M0005ListingSortName],
    vec_box![
        // What each product costs in each currency it is sold in — the listed
        // one included — so a listing can be asked for in one currency.
        (
            "CREATE TABLE catalog_listing_currency_price (
                product_id TEXT NOT NULL,
                currency TEXT NOT NULL,
                price_minor INTEGER NOT NULL,
                PRIMARY KEY (product_id, currency)
            )",
            "DROP TABLE catalog_listing_currency_price"
        ),
        (
            "CREATE INDEX catalog_listing_currency_price_amount
             ON catalog_listing_currency_price (currency, price_minor)",
            "DROP INDEX catalog_listing_currency_price_amount"
        ),
        // The listed prices known so far; the others come with the next
        // refresh of each product.
        (
            "INSERT INTO catalog_listing_currency_price (product_id, currency, price_minor)
             SELECT product_id, currency, price_minor FROM catalog_listing
             WHERE price_minor IS NOT NULL AND currency IS NOT NULL",
            "DELETE FROM catalog_listing_currency_price"
        )
    ]
);

pub struct M0007Families;

sqlite_migration!(
    M0007Families,
    "catalog",
    "m0007_families",
    vec_box![M0006ListingPrices],
    vec_box![
        (
            "CREATE TABLE catalog_family (
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                options TEXT NOT NULL DEFAULT '',
                variant_count INTEGER NOT NULL DEFAULT 0,
                dissolved INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE catalog_family"
        ),
        // `position`: the order the variants joined in.
        (
            "CREATE TABLE catalog_family_variant (
                family_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                option_values TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (family_id, product_id)
            )",
            "DROP TABLE catalog_family_variant"
        ),
        (
            "CREATE INDEX catalog_family_variant_product ON catalog_family_variant (product_id)",
            "DROP INDEX catalog_family_variant_product"
        )
    ]
);

pub struct M0008ListingFamilies;

sqlite_migration!(
    M0008ListingFamilies,
    "catalog",
    "m0008_listing_families",
    vec_box![M0007Families],
    vec_box![
        // The family a product is a version of, and its name: a listing shows
        // one card per family.
        (
            "ALTER TABLE catalog_listing ADD COLUMN family_id TEXT",
            "ALTER TABLE catalog_listing DROP COLUMN family_id"
        ),
        (
            "ALTER TABLE catalog_listing ADD COLUMN family_name TEXT",
            "ALTER TABLE catalog_listing DROP COLUMN family_name"
        ),
        (
            "CREATE INDEX catalog_listing_family ON catalog_listing (family_id)",
            "DROP INDEX catalog_listing_family"
        ),
        // The variants placed so far; the others come with the next refresh
        // of each product.
        (
            "UPDATE catalog_listing
             SET family_id = (SELECT v.family_id FROM catalog_family_variant v
                              WHERE v.product_id = catalog_listing.product_id),
                 family_name = (SELECT f.name FROM catalog_family_variant v
                                JOIN catalog_family f ON f.id = v.family_id
                                WHERE v.product_id = catalog_listing.product_id)",
            "UPDATE catalog_listing SET family_id = NULL, family_name = NULL"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![
        M0001CatalogProduct,
        M0002Categories,
        M0003Listing,
        M0004SpecFacets,
        M0005ListingSortName,
        M0006ListingPrices,
        M0007Families,
        M0008ListingFamilies
    ]
}
