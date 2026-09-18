use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001OrderHistory;

sqlite_migration!(
    M0001OrderHistory,
    "order",
    "m0001_order_history",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE order_history (
                order_id TEXT PRIMARY KEY,
                customer_id TEXT NOT NULL,
                placed_at INTEGER NOT NULL,
                year INTEGER NOT NULL,
                seller TEXT NOT NULL,
                status TEXT NOT NULL,
                total_minor INTEGER NOT NULL,
                currency TEXT NOT NULL
            )",
            "DROP TABLE order_history"
        ),
        (
            "CREATE INDEX order_history_customer_year ON order_history (customer_id, year, placed_at)",
            "DROP INDEX order_history_customer_year"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001OrderHistory]
}
