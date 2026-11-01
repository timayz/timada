//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "invoicing";

/// The read models of this context, one table each (see the columns).
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_documents",
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
            );
        CREATE INDEX invoicing_documents_issued ON invoicing_documents (tenant_id, issued_at);
        CREATE INDEX invoicing_documents_order ON invoicing_documents (tenant_id, order_id);
        CREATE TABLE invoicing_vat_lines (
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
            );
        CREATE INDEX invoicing_vat_lines_issued ON invoicing_vat_lines (tenant_id, regime, issued_at);
        CREATE INDEX invoicing_vat_lines_company ON invoicing_vat_lines (company_id, regime, issued_at);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &["invoicing_documents", "invoicing_vat_lines"];
