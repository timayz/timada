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

pub struct M0002StockList;

sqlite_migration!(
    M0002StockList,
    "inventory",
    "m0002_stock_list",
    vec_box![M0001BackInStockAlert],
    vec_box![
        (
            "CREATE TABLE inventory_stock_list (
                stock_item_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                location TEXT NOT NULL,
                on_hand INTEGER NOT NULL,
                reserved INTEGER NOT NULL,
                available INTEGER NOT NULL
            )",
            "DROP TABLE inventory_stock_list"
        ),
        (
            "CREATE INDEX inventory_stock_list_available ON inventory_stock_list (available)",
            "DROP INDEX inventory_stock_list_available"
        )
    ]
);

pub struct M0003AlertList;

sqlite_migration!(
    M0003AlertList,
    "inventory",
    "m0003_alert_list",
    vec_box![M0002StockList],
    vec_box![
        (
            "CREATE TABLE inventory_alert_list (
                alert_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                requested_at INTEGER NOT NULL,
                triggered_at INTEGER
            )",
            "DROP TABLE inventory_alert_list"
        ),
        (
            "CREATE INDEX inventory_alert_list_customer
             ON inventory_alert_list (customer_id, requested_at)",
            "DROP INDEX inventory_alert_list_customer"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001BackInStockAlert, M0002StockList, M0003AlertList]
}
