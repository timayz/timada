use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "automations";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Automations0001]
}

pub struct Automations0001;
sqlx_migrator::sqlite_migration!(
    Automations0001,
    "automations",
    "0001_scenarios",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE automations_scenarios (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                name TEXT NOT NULL,
                status TEXT NOT NULL,
                trigger TEXT,
                version INTEGER NOT NULL,
                unpublished INTEGER NOT NULL,
                steps INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE automations_scenarios"
        ),
        (
            "CREATE INDEX automations_scenarios_updated ON automations_scenarios
                (tenant_id, updated_at DESC)",
            "DROP INDEX automations_scenarios_updated"
        ),
    ]
);
