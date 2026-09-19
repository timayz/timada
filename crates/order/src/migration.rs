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

pub struct M0002OrderNumber;

sqlite_migration!(
    M0002OrderNumber,
    "order",
    "m0002_order_number",
    vec_box![M0001OrderHistory],
    vec_box![
        (
            "CREATE TABLE order_number (
                order_id TEXT PRIMARY KEY,
                number INTEGER NOT NULL UNIQUE,
                year INTEGER NOT NULL
            )",
            "DROP TABLE order_number"
        ),
        (
            "ALTER TABLE order_history ADD COLUMN order_number TEXT",
            "ALTER TABLE order_history DROP COLUMN order_number"
        ),
        (
            "CREATE INDEX order_history_order_number ON order_history (order_number)",
            "DROP INDEX order_history_order_number"
        )
    ]
);

pub struct M0003AwaitingPayment;

sqlite_migration!(
    M0003AwaitingPayment,
    "order",
    "m0003_awaiting_payment",
    vec_box![M0002OrderNumber],
    vec_box![
        (
            "CREATE TABLE order_awaiting_payment (
                order_id TEXT PRIMARY KEY,
                payment_id TEXT NOT NULL,
                since INTEGER NOT NULL
            )",
            "DROP TABLE order_awaiting_payment"
        ),
        (
            "CREATE INDEX order_awaiting_payment_since ON order_awaiting_payment (since)",
            "DROP INDEX order_awaiting_payment_since"
        )
    ]
);

/// Write-side and read-model migrations for this context, to register
/// alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001OrderHistory, M0002OrderNumber, M0003AwaitingPayment]
}
