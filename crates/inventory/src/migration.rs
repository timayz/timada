use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001BackInStockAlert;

sqlite_migration!(
    M0001BackInStockAlert,
    "inventory",
    "m0001_back_in_stock_alert",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE inventory_back_in_stock_alert (
                alert_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                triggered INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE inventory_back_in_stock_alert"
        ),
        (
            "CREATE INDEX inventory_back_in_stock_alert_product
             ON inventory_back_in_stock_alert (product_id, triggered)",
            "DROP INDEX inventory_back_in_stock_alert_product"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001BackInStockAlert]
}
