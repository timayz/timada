use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "catalog";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Products0001, Categories0002]
}

pub struct Products0001;
sqlx_migrator::sqlite_migration!(
    Products0001,
    "catalog",
    "0001_products",
    vec_box![],
    vec_box![
        (
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
    ]
);

pub struct Categories0002;
sqlx_migrator::sqlite_migration!(
    Categories0002,
    "catalog",
    "0002_categories",
    vec_box![("catalog", "0001_products")],
    vec_box![
        (
            "CREATE TABLE catalog_categories (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                slug TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                parent_id TEXT,
                position INTEGER NOT NULL,
                archived INTEGER NOT NULL,
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
    ]
);
