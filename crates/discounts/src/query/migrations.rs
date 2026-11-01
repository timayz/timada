//! Schema of the read model: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "discounts";

/// `discounts_codes`: one row per discount of a tenant.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_discounts",
        "CREATE TABLE discounts_codes (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            code TEXT NOT NULL,
            percent_bps INTEGER,
            amount_cents INTEGER,
            currency TEXT,
            starts_at INTEGER,
            ends_at INTEGER,
            max_uses INTEGER,
            once_per_email INTEGER NOT NULL,
            status TEXT NOT NULL,
            created_by TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, id)
        );
        CREATE UNIQUE INDEX discounts_codes_code ON discounts_codes (tenant_id, code)
            WHERE status <> 'archived';",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &["discounts_codes"];
