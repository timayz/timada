//! Schema of the read model: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "support";

/// `support_messages`: one row per message of a tenant, opened.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_messages",
        "CREATE TABLE support_messages (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            email TEXT NOT NULL,
            body TEXT NOT NULL,
            language TEXT,
            status TEXT NOT NULL,
            received_at INTEGER NOT NULL,
            handled_at INTEGER,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, id)
        );
        CREATE INDEX support_messages_status ON support_messages
            (tenant_id, status, received_at DESC);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &["support_messages"];
