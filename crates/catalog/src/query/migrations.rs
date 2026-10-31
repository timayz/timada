use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "catalog";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Catalog0001]
}

pub struct Catalog0001;
sqlx_migrator::sqlite_migration!(
    Catalog0001,
    "catalog",
    "0001_products",
    vec_box![],
    vec_box![
        (
            // One row per product of a tenant. `thumbnail` is the first
            // picture's file name, for a listing's thumbnail. When a product
            // varies, `min_amount_cents` and `max_amount_cents` are the
            // cheapest and the dearest a shopper may pay. `stock_state` is
            // worked out from the stock rows after the row is written, hence
            // its default.
            "CREATE TABLE catalog_products (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                slug TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                amount_cents INTEGER,
                currency TEXT,
                status TEXT NOT NULL,
                created_by TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                thumbnail TEXT,
                image_count INTEGER NOT NULL,
                min_amount_cents INTEGER,
                max_amount_cents INTEGER,
                variant_count INTEGER NOT NULL,
                stock_state TEXT NOT NULL DEFAULT 'untracked',
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE catalog_products"
        ),
        (
            "CREATE UNIQUE INDEX catalog_products_slug ON catalog_products (tenant_id, slug)
                WHERE status <> 'archived'",
            "DROP INDEX catalog_products_slug"
        ),
        (
            "CREATE INDEX catalog_products_name ON catalog_products (tenant_id, name)",
            "DROP INDEX catalog_products_name"
        ),
        (
            // Cut in runs of three characters, so a search matches anywhere
            // in a word as `LIKE '%q%'` would, ignoring case and accents. The
            // tenant is a column too, so a search narrows to one tenant's
            // products inside the index. The rowid is the product row's.
            "CREATE VIRTUAL TABLE catalog_products_fts USING fts5(
                tenant_id, slug, name, tokenize = 'trigram remove_diacritics 1'
            )",
            "DROP TABLE catalog_products_fts"
        ),
        (
            // The counts are refreshed from the product rows, after the
            // category's own row is written.
            "CREATE TABLE catalog_categories (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                slug TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                parent_id TEXT,
                position INTEGER NOT NULL,
                archived INTEGER NOT NULL,
                in_menu INTEGER NOT NULL,
                created_by TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                product_count INTEGER NOT NULL DEFAULT 0,
                subtree_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE catalog_categories"
        ),
        (
            "CREATE UNIQUE INDEX catalog_categories_slug ON catalog_categories (tenant_id, slug)
                WHERE archived = 0",
            "DROP INDEX catalog_categories_slug"
        ),
        (
            "CREATE INDEX catalog_categories_parent
                ON catalog_categories (tenant_id, parent_id, position, name)",
            "DROP INDEX catalog_categories_parent"
        ),
        (
            "CREATE TABLE catalog_product_categories (
                tenant_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                category_id TEXT NOT NULL,
                PRIMARY KEY (tenant_id, product_id, category_id)
            )",
            "DROP TABLE catalog_product_categories"
        ),
        (
            "CREATE INDEX catalog_product_categories_category
                ON catalog_product_categories (tenant_id, category_id)",
            "DROP INDEX catalog_product_categories_category"
        ),
        (
            // One row per thing the shop counts: a product without variants
            // (`variant_id` '') or one variant. What checkouts hold is left
            // out: it lapses on its own, and the pages read the stream.
            "CREATE TABLE catalog_stock (
                tenant_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                variant_id TEXT NOT NULL DEFAULT '',
                tracked INTEGER NOT NULL,
                on_hand INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, product_id, variant_id)
            )",
            "DROP TABLE catalog_stock"
        ),
    ]
);
