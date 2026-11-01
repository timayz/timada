//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "shoppers";

/// The read models of this context, one table each (see the columns).
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_shoppers",
        "CREATE TABLE shoppers_shoppers (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                email TEXT NOT NULL,
                registered_at INTEGER NOT NULL,
                address_count INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX shoppers_shoppers_registered ON shoppers_shoppers
                (tenant_id, registered_at DESC);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &["shoppers_shoppers"];
