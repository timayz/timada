use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "invoicing";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Invoicing0001, Invoicing0002, Invoicing0003]
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

pub struct Invoicing0002;
sqlx_migrator::sqlite_migration!(
    Invoicing0002,
    "invoicing",
    "0002_vat_by_destination",
    vec_box![("invoicing", "0001_documents")],
    vec_box![
        (
            "ALTER TABLE invoicing_documents ADD COLUMN regime TEXT NOT NULL DEFAULT 'domestic'",
            "ALTER TABLE invoicing_documents DROP COLUMN regime"
        ),
        (
            "ALTER TABLE invoicing_documents ADD COLUMN tax_country TEXT NOT NULL DEFAULT 'FR'",
            "ALTER TABLE invoicing_documents DROP COLUMN tax_country"
        ),
        (
            "ALTER TABLE invoicing_documents ADD COLUMN eu_distance_sales_cents INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE invoicing_documents DROP COLUMN eu_distance_sales_cents"
        ),
        (
            "ALTER TABLE invoicing_documents ADD COLUMN eu_threshold_passed INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE invoicing_documents DROP COLUMN eu_threshold_passed"
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
                PRIMARY KEY (tenant_id, document_id, rate_bps)
            )",
            "DROP TABLE invoicing_vat_lines"
        ),
        (
            "CREATE INDEX invoicing_vat_lines_issued ON invoicing_vat_lines (tenant_id, regime, issued_at)",
            "DROP INDEX invoicing_vat_lines_issued"
        ),
    ]
);

pub struct Invoicing0003;
sqlx_migrator::sqlite_migration!(
    Invoicing0003,
    "invoicing",
    "0003_companies",
    vec_box![("invoicing", "0002_vat_by_destination")],
    vec_box![
        (
            "ALTER TABLE invoicing_documents ADD COLUMN company_id TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE invoicing_documents DROP COLUMN company_id"
        ),
        (
            "ALTER TABLE invoicing_vat_lines ADD COLUMN company_id TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE invoicing_vat_lines DROP COLUMN company_id"
        ),
        (
            "CREATE INDEX invoicing_vat_lines_company ON invoicing_vat_lines (company_id, regime, issued_at)",
            "DROP INDEX invoicing_vat_lines_company"
        ),
    ]
);
