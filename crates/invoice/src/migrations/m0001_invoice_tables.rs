use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001InvoiceTables;

sqlite_migration!(
    M0001InvoiceTables,
    "invoice",
    "m0001_invoice_tables",
    vec_box![],
    vec_box![
        // Not a read model: this counter is write-side state, the one thing an
        // event store cannot give us (see `crate::numbering`).
        (
            "CREATE TABLE invoice_sequences (
                 kind TEXT PRIMARY KEY NOT NULL,
                 next_value INTEGER NOT NULL
             )",
            "DROP TABLE invoice_sequences"
        ),
        (
            "CREATE TABLE admin_invoice_list (
                 id TEXT PRIMARY KEY NOT NULL,
                 order_id TEXT NOT NULL,
                 number TEXT NOT NULL,
                 total_gross_cents INTEGER NOT NULL,
                 currency TEXT NOT NULL,
                 status TEXT NOT NULL,
                 credit_note_number TEXT,
                 created_at INTEGER NOT NULL
             )",
            "DROP TABLE admin_invoice_list"
        ),
        (
            "CREATE INDEX idx_admin_invoice_list_recent
                 ON admin_invoice_list (created_at DESC, id DESC)",
            "DROP INDEX idx_admin_invoice_list_recent"
        )
    ]
);
