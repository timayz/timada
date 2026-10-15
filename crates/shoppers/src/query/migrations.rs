use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "shoppers";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Shoppers0001]
}

pub struct Shoppers0001;
sqlx_migrator::sqlite_migration!(
    Shoppers0001,
    "shoppers",
    "0001_shoppers",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE shoppers_shoppers (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                email TEXT NOT NULL,
                registered_at INTEGER NOT NULL,
                address_count INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE shoppers_shoppers"
        ),
        (
            "CREATE INDEX shoppers_shoppers_registered ON shoppers_shoppers
                (tenant_id, registered_at DESC)",
            "DROP INDEX shoppers_shoppers_registered"
        ),
    ]
);
