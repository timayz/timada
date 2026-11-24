//! Schema of the read model, applied by the composition root in the
//! store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "proposals";

/// `proposals`: one row per proposal of a tenant. `waiting` is 1 until it
/// is answered or found out of date, and `listed_at` is where it sorts in
/// its list: when it was prepared while it waits, when it ended after. The
/// index gives either list in its order, so a page reads its rows and no
/// others. `changes` is JSON: each field with its value before and after.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_proposals",
        "CREATE TABLE proposals (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            kind TEXT NOT NULL,
            subject_id TEXT NOT NULL,
            subject_name TEXT NOT NULL,
            source TEXT NOT NULL,
            prepared_by TEXT NOT NULL,
            reason TEXT NOT NULL,
            changes TEXT NOT NULL,
            edited INTEGER NOT NULL,
            status TEXT NOT NULL,
            waiting INTEGER NOT NULL,
            valid_until INTEGER NOT NULL,
            prepared_at INTEGER NOT NULL,
            answered_by TEXT NOT NULL,
            note TEXT NOT NULL,
            listed_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, id)
        );
        CREATE INDEX proposals_listed ON proposals
            (tenant_id, waiting, listed_at DESC, id DESC);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &["proposals"];
