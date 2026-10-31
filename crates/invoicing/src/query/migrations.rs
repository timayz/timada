use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "invoicing";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Invoicing0001]
}

/// The documents a shop issued, with the VAT regime and country each one was
/// taxed under and the company it was issued by, and one VAT line per rate on
/// each document for the ledgers.
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
                regime TEXT NOT NULL,
                tax_country TEXT NOT NULL,
                eu_distance_sales_cents INTEGER NOT NULL,
                eu_threshold_passed INTEGER NOT NULL,
                company_id TEXT NOT NULL,
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
        (
            "CREATE TABLE invoicing_vat_lines (
                tenant_id TEXT NOT NULL,
                document_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                regime TEXT NOT NULL,
                country TEXT NOT NULL,
                rate_bps INTEGER NOT NULL,
                base_cents INTEGER NOT NULL,
                vat_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                company_id TEXT NOT NULL,
                PRIMARY KEY (tenant_id, document_id, rate_bps)
            )",
            "DROP TABLE invoicing_vat_lines"
        ),
        (
            "CREATE INDEX invoicing_vat_lines_issued ON invoicing_vat_lines (tenant_id, regime, issued_at)",
            "DROP INDEX invoicing_vat_lines_issued"
        ),
        (
            "CREATE INDEX invoicing_vat_lines_company ON invoicing_vat_lines (company_id, regime, issued_at)",
            "DROP INDEX invoicing_vat_lines_company"
        ),
    ]
);
