use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001InvoiceNumber;

sqlite_migration!(
    M0001InvoiceNumber,
    "invoice",
    "m0001_invoice_number",
    vec_box![],
    vec_box![(
        "CREATE TABLE invoice_number (
            order_id TEXT PRIMARY KEY,
            number INTEGER NOT NULL UNIQUE
        )",
        "DROP TABLE invoice_number"
    )]
);

pub struct M0002InvoiceList;

sqlite_migration!(
    M0002InvoiceList,
    "invoice",
    "m0002_invoice_list",
    vec_box![M0001InvoiceNumber],
    vec_box![
        (
            "CREATE TABLE invoice_list (
                invoice_id TEXT PRIMARY KEY,
                order_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                invoice_number TEXT,
                status TEXT NOT NULL,
                total_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                drafted_at INTEGER NOT NULL,
                issued_at INTEGER
            )",
            "DROP TABLE invoice_list"
        ),
        (
            "CREATE INDEX invoice_list_status ON invoice_list (status, drafted_at)",
            "DROP INDEX invoice_list_status"
        )
    ]
);

/// Write-side and read-model migrations for this context, to register
/// alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001InvoiceNumber, M0002InvoiceList]
}
