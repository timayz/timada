use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001ReturnClaim;

sqlite_migration!(
    M0001ReturnClaim,
    "returns",
    "m0001_return_claim",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE return_number (
                number INTEGER PRIMARY KEY AUTOINCREMENT,
                year INTEGER NOT NULL,
                order_id TEXT NOT NULL
            )",
            "DROP TABLE return_number"
        ),
        (
            "CREATE TABLE return_claim (
                return_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                quantity INTEGER NOT NULL,
                PRIMARY KEY (return_id, product_id)
            )",
            "DROP TABLE return_claim"
        ),
        (
            "CREATE INDEX return_claim_order ON return_claim (order_id, product_id)",
            "DROP INDEX return_claim_order"
        )
    ]
);

pub struct M0002ReturnList;

sqlite_migration!(
    M0002ReturnList,
    "returns",
    "m0002_return_list",
    vec_box![M0001ReturnClaim],
    vec_box![
        (
            "CREATE TABLE return_list (
                return_id TEXT PRIMARY KEY,
                rma_number TEXT NOT NULL,
                order_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                status TEXT NOT NULL,
                reason TEXT NOT NULL,
                units INTEGER NOT NULL,
                refunded_minor INTEGER NOT NULL,
                credited_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                requested_at INTEGER NOT NULL
            )",
            "DROP TABLE return_list"
        ),
        (
            "CREATE INDEX return_list_status ON return_list (status, requested_at)",
            "DROP INDEX return_list_status"
        ),
        (
            "CREATE INDEX return_list_order ON return_list (order_id, requested_at)",
            "DROP INDEX return_list_order"
        )
    ]
);

/// Write-side and read-model migrations for this context, to register
/// alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001ReturnClaim, M0002ReturnList]
}
