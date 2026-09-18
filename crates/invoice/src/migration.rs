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

/// Write-side migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001InvoiceNumber]
}
