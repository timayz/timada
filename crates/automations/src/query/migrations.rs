//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "automations";

/// The read models of this context, one table each (see the columns).
///
/// A scenario's run history is read a page at a time off the index of its
/// runs, or of its runs of one status, and its counts are kept as the rows
/// change (`automations_run_counts`, the runs listed by scenario and
/// status), never counted over the runs.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_automations",
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
            );
        CREATE INDEX automations_scenarios_updated ON automations_scenarios
                (tenant_id, updated_at DESC);
        CREATE TABLE automations_runs (
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
                paused INTEGER NOT NULL,
                shard INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX automations_runs_due ON automations_runs (next_at, id)
                WHERE next_at IS NOT NULL AND paused = 0;
        CREATE INDEX automations_runs_scenario ON automations_runs
                (tenant_id, scenario_id, started_at, id);
        CREATE INDEX automations_runs_scenario_status ON automations_runs
                (tenant_id, scenario_id, status, started_at, id);
        CREATE INDEX automations_runs_ended ON automations_runs (updated_at)
                WHERE next_at IS NULL;
        CREATE TABLE automations_run_counts (
                tenant_id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                status TEXT NOT NULL,
                runs INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, scenario_id, status)
            );
        CREATE TABLE automations_versions (
                tenant_id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                definition TEXT NOT NULL,
                published_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, scenario_id, version)
            );
        CREATE TABLE automations_listeners (
                tenant_id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                trigger TEXT NOT NULL,
                version INTEGER NOT NULL,
                state TEXT NOT NULL,
                since INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, scenario_id)
            );
        CREATE INDEX automations_listeners_trigger ON automations_listeners
                (tenant_id, trigger, state);
        CREATE TABLE automations_checkouts (
                tenant_id TEXT NOT NULL,
                cart_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                requested_at INTEGER NOT NULL,
                region TEXT,
                ordered INTEGER NOT NULL,
                detected INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, cart_id)
            );
        CREATE INDEX automations_checkouts_open ON automations_checkouts (requested_at)
                WHERE ordered = 0 AND detected = 0;
        CREATE TABLE automations_starts (
                tenant_id TEXT NOT NULL,
                run_id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                scenario_version INTEGER NOT NULL,
                trigger TEXT NOT NULL,
                trigger_event_id TEXT NOT NULL,
                subject_id TEXT NOT NULL,
                kept_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, run_id)
            );
        CREATE INDEX automations_starts_kept ON automations_starts (kept_at);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &[
    "automations_scenarios",
    "automations_runs",
    "automations_run_counts",
    "automations_versions",
    "automations_listeners",
    "automations_checkouts",
    "automations_starts",
];
