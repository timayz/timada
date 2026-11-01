//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "automations";

/// The read models of this context, one table each (see the columns).
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
                started_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX automations_runs_due ON automations_runs (next_at)
                WHERE next_at IS NOT NULL;
        CREATE INDEX automations_runs_scenario ON automations_runs
                (tenant_id, scenario_id, started_at DESC);
        CREATE INDEX automations_runs_ended ON automations_runs (updated_at)
                WHERE next_at IS NULL;
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
                ordered INTEGER NOT NULL,
                detected INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, cart_id)
            );
        CREATE INDEX automations_checkouts_open ON automations_checkouts (requested_at)
                WHERE ordered = 0 AND detected = 0;",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &[
    "automations_scenarios",
    "automations_runs",
    "automations_listeners",
    "automations_checkouts",
];
