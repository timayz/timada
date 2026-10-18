use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "aliexpress";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Products0001]
}

pub struct Products0001;
sqlx_migrator::sqlite_migration!(
    Products0001,
    "aliexpress",
    "0001_products",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE aliexpress_products (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                ae_product_id TEXT NOT NULL,
                product_id TEXT,
                status TEXT NOT NULL,
                offline INTEGER NOT NULL,
                own_pricing INTEGER NOT NULL,
                last_synced_at INTEGER,
                last_error TEXT,
                requested_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE aliexpress_products"
        ),
        (
            "CREATE INDEX aliexpress_products_product ON aliexpress_products (tenant_id, product_id)",
            "DROP INDEX aliexpress_products_product"
        ),
        (
            "CREATE INDEX aliexpress_products_due ON aliexpress_products (status, last_synced_at)",
            "DROP INDEX aliexpress_products_due"
        ),
    ]
);
