use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001RefundList;

sqlite_migration!(
    M0001RefundList,
    "payment",
    "m0001_refund_list",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE payment_refund_list (
                refund_id TEXT PRIMARY KEY,
                payment_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                amount_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                reason TEXT NOT NULL,
                refunded_at INTEGER NOT NULL
            )",
            "DROP TABLE payment_refund_list"
        ),
        (
            "CREATE INDEX payment_refund_list_refunded_at
             ON payment_refund_list (refunded_at)",
            "DROP INDEX payment_refund_list_refunded_at"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001RefundList]
}
