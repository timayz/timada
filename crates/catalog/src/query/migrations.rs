use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "catalog";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Products0001]
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
