use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001SavedList;

sqlite_migration!(
    M0001SavedList,
    "cart",
    "m0001_saved_list",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE cart_saved_list (
                cart_id TEXT PRIMARY KEY,
                customer_id TEXT NOT NULL,
                name TEXT NOT NULL,
                units INTEGER NOT NULL,
                subtotal_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                saved_at INTEGER NOT NULL
            )",
            "DROP TABLE cart_saved_list"
        ),
        (
            "CREATE INDEX cart_saved_list_customer ON cart_saved_list (customer_id, saved_at)",
            "DROP INDEX cart_saved_list_customer"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001SavedList]
}
