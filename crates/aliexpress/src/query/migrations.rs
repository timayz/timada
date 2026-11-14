//! Schema of the read models, and of this node's own tables, applied by the
//! composition root in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "aliexpress";

/// The read models of this context, one table each (see the columns).
/// `aliexpress_products.skus` is the item's combinations as JSON (a list of
/// [`SkuLink`](crate::aggregator::SkuLink)): which one each variant of the
/// catalog product was imported from, so a page tells an AliExpress line
/// from the row. An order has one forward (its id is made of the order's),
/// which the index on `aliexpress_forwards` holds to.
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
                shard INTEGER NOT NULL,
                wanted_at INTEGER,
                published INTEGER NOT NULL DEFAULT 0,
                skus TEXT NOT NULL DEFAULT '[]',
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
                shard INTEGER NOT NULL,
                settling INTEGER NOT NULL,
                state TEXT NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX aliexpress_forwards_due ON aliexpress_forwards (status, updated_at);
        CREATE UNIQUE INDEX aliexpress_forwards_order ON aliexpress_forwards (tenant_id, order_id);",
    )],
);

/// The read-model tables the migrations create: what a node drops and creates
/// again before copying its store from a peer, so the subscriptions fill them
/// again. The node's own tables ([`NODE_MIGRATIONS`]) are not among them.
pub const TABLES: &[&str] = &["aliexpress_products", "aliexpress_forwards"];

/// The app name under which this node's own tables are recorded.
pub const NODE_MIGRATION_APP: &str = "aliexpress_node";

/// This node's own tables, which no peer holds and a copy of the store from
/// a peer leaves alone, recorded apart from the read models for that:
/// `aliexpress_read_times`, when this node last read each item and each
/// open order on AliExpress, and whether a shipped order's parcels are
/// followed any more ([`super::read_times`]); `aliexpress_deliveries`, what
/// delivering each item last cost, for the reads that reuse it
/// ([`super::deliveries`]); and `aliexpress_unrecorded`, what AliExpress took
/// and the store would not take yet ([`super::unrecorded`]).
pub const NODE_MIGRATIONS: Migrations<'static> = Migrations::new(
    NODE_MIGRATION_APP,
    &[Migration::up(
        "0001_read_times_unrecorded",
        "CREATE TABLE aliexpress_read_times (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                tried_at INTEGER,
                synced_at INTEGER,
                delivered INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE TABLE aliexpress_deliveries (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                sku_id TEXT NOT NULL,
                sku_cost_cents INTEGER NOT NULL,
                fee_cents INTEGER NOT NULL,
                min_days INTEGER,
                max_days INTEGER,
                ships_from TEXT,
                read_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE TABLE aliexpress_unrecorded (
                tenant_id TEXT NOT NULL,
                forward_id TEXT NOT NULL,
                ae_order_ids TEXT NOT NULL,
                order_id TEXT NOT NULL,
                placement TEXT NOT NULL,
                placed_at INTEGER NOT NULL,
                tries INTEGER NOT NULL,
                last_error TEXT,
                PRIMARY KEY (tenant_id, forward_id, ae_order_ids)
            );",
    )],
);
