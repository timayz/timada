use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "discounts";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Discounts0001]
}

pub struct Discounts0001;
sqlx_migrator::sqlite_migration!(
    Discounts0001,
    "discounts",
    "0001_discounts",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE discounts_codes (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                code TEXT NOT NULL,
                percent_bps INTEGER,
                amount_cents INTEGER,
                currency TEXT,
                starts_at INTEGER,
                ends_at INTEGER,
                max_uses INTEGER,
                once_per_email INTEGER NOT NULL,
                status TEXT NOT NULL,
                created_by TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE discounts_codes"
        ),
        (
            "CREATE UNIQUE INDEX discounts_codes_code ON discounts_codes (tenant_id, code)
                WHERE status <> 'archived'",
            "DROP INDEX discounts_codes_code"
        ),
    ]
);
