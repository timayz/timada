//! Schema of the table of events the sagas gave up on, on its own so the
//! integration tests that include `db.rs` by path can include it too.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which the table's migrations are recorded.
pub const MIGRATION_APP: &str = "saga_fault";

/// `saga_given_up`: one row per event a saga of this node gave up on, kept
/// a week (see [`KEPT_SECS`](super::KEPT_SECS)).
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_saga_given_up",
        "CREATE TABLE saga_given_up (
            subscription TEXT NOT NULL,
            tenant_id TEXT NOT NULL,
            stream_id TEXT NOT NULL,
            event TEXT NOT NULL,
            reason TEXT NOT NULL,
            given_up_at INTEGER NOT NULL,
            PRIMARY KEY (subscription, tenant_id, stream_id, event)
        );",
    )],
);

/// The tables the migrations create: what a node empties before copying its
/// store from a peer.
pub const TABLES: &[&str] = &["saga_given_up"];
