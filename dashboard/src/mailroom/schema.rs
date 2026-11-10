//! Schema of the mailroom's table, on its own so the integration tests that
//! include `db.rs` by path can include it too.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which the table's migrations are recorded.
pub const MIGRATION_APP: &str = "mailroom";

/// `owed_mails`: one row per stream whose latest intent owes a mail this
/// node has not seen settled.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[
        Migration::up(
            "0001_owed_mails",
            "CREATE TABLE owed_mails (
                kind TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                stream_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                due_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                attempts INTEGER NOT NULL,
                failed INTEGER NOT NULL,
                reason TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (kind, tenant_id, stream_id)
            );",
        ),
        Migration::up(
            "0002_owed_mails_due",
            "CREATE INDEX owed_mails_due ON owed_mails (failed, due_at);",
        ),
    ],
);

/// The tables the migrations create: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &["owed_mails"];
