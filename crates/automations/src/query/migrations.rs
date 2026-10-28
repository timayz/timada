use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "automations";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Automations0001, Automations0002, Automations0003]
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

pub struct Automations0002;
sqlx_migrator::sqlite_migration!(
    Automations0002,
    "automations",
    "0002_runs",
    vec_box![Automations0001],
    vec_box![
        (
            "CREATE TABLE automations_runs (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                scenario_version INTEGER NOT NULL,
                trigger TEXT NOT NULL,
                subject_id TEXT NOT NULL,
                status TEXT NOT NULL,
                step TEXT,
                reason TEXT,
                next_at INTEGER,
                started_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE automations_runs"
        ),
        (
            "CREATE INDEX automations_runs_due ON automations_runs (next_at)
                WHERE next_at IS NOT NULL",
            "DROP INDEX automations_runs_due"
        ),
        (
            "CREATE INDEX automations_runs_scenario ON automations_runs
                (tenant_id, scenario_id, started_at DESC)",
            "DROP INDEX automations_runs_scenario"
        ),
        (
            "CREATE INDEX automations_runs_ended ON automations_runs (updated_at)
                WHERE next_at IS NULL",
            "DROP INDEX automations_runs_ended"
        ),
        (
            "CREATE TABLE automations_listeners (
                tenant_id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                trigger TEXT NOT NULL,
                version INTEGER NOT NULL,
                state TEXT NOT NULL,
                since INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, scenario_id)
            )",
            "DROP TABLE automations_listeners"
        ),
        (
            "CREATE INDEX automations_listeners_trigger ON automations_listeners
                (tenant_id, trigger, state)",
            "DROP INDEX automations_listeners_trigger"
        ),
    ]
);

pub struct Automations0003;
sqlx_migrator::sqlite_migration!(
    Automations0003,
    "automations",
    "0003_checkouts",
    vec_box![Automations0002],
    vec_box![
        (
            "CREATE TABLE automations_checkouts (
                tenant_id TEXT NOT NULL,
                cart_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                requested_at INTEGER NOT NULL,
                ordered INTEGER NOT NULL,
                detected INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, cart_id)
            )",
            "DROP TABLE automations_checkouts"
        ),
        (
            "CREATE INDEX automations_checkouts_open ON automations_checkouts (requested_at)
                WHERE ordered = 0 AND detected = 0",
            "DROP INDEX automations_checkouts_open"
        ),
    ]
);
