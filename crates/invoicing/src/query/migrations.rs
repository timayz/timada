use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "invoicing";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Invoicing0001]
}

pub struct Invoicing0001;
sqlx_migrator::sqlite_migration!(
    Invoicing0001,
    "invoicing",
    "0001_documents",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE invoicing_documents (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                kind TEXT NOT NULL,
                series TEXT NOT NULL,
                number INTEGER NOT NULL,
                label TEXT NOT NULL,
                order_id TEXT NOT NULL,
                order_reference TEXT NOT NULL,
                buyer_name TEXT NOT NULL,
                total_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE invoicing_documents"
        ),
        (
            "CREATE INDEX invoicing_documents_issued ON invoicing_documents (tenant_id, issued_at)",
            "DROP INDEX invoicing_documents_issued"
        ),
        (
            "CREATE INDEX invoicing_documents_order ON invoicing_documents (tenant_id, order_id)",
            "DROP INDEX invoicing_documents_order"
        ),
    ]
);
