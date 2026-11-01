//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "aliexpress";

/// The read models of this context, one table each (see the columns).
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_products",
        "CREATE TABLE aliexpress_products (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                ae_product_id TEXT NOT NULL,
                product_id TEXT,
                status TEXT NOT NULL,
                offline INTEGER NOT NULL,
                own_pricing INTEGER NOT NULL,
                last_synced_at INTEGER,
                last_error TEXT,
                requested_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                new_combinations INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX aliexpress_products_product ON aliexpress_products (tenant_id, product_id);
        CREATE INDEX aliexpress_products_due ON aliexpress_products (status, last_synced_at);
        CREATE TABLE aliexpress_forwards (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                status TEXT NOT NULL,
                ae_order_ids TEXT NOT NULL,
                tracking TEXT NOT NULL,
                last_error TEXT,
                placed_at INTEGER,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX aliexpress_forwards_due ON aliexpress_forwards (status, updated_at);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &["aliexpress_products", "aliexpress_forwards"];
