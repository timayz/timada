use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "aliexpress";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Aliexpress0001]
}

pub struct Aliexpress0001;
sqlx_migrator::sqlite_migration!(
    Aliexpress0001,
    "aliexpress",
    "0001_products",
    vec_box![],
    vec_box![
        (
            // One row per AliExpress product a tenant imports, and the
            // catalogue product it became. `new_combinations` counts the
            // combinations the last read found that the shop does not sell,
            // for the merchant to offer or ignore.
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
                new_combinations INTEGER NOT NULL,
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
        (
            // One row per order forwarded to AliExpress, with the orders it
            // placed there and the tracking numbers they reported.
            "CREATE TABLE aliexpress_forwards (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                status TEXT NOT NULL,
                ae_order_ids TEXT NOT NULL,
                tracking TEXT NOT NULL,
                last_error TEXT,
                placed_at INTEGER,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE aliexpress_forwards"
        ),
        (
            "CREATE INDEX aliexpress_forwards_due ON aliexpress_forwards (status, updated_at)",
            "DROP INDEX aliexpress_forwards_due"
        ),
    ]
);
