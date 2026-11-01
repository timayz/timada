//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "orders";

/// The read models of this context, one table each (see the columns).
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_orders_and_carts",
        "CREATE TABLE orders_orders (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                reference TEXT NOT NULL,
                cart_id TEXT NOT NULL,
                status TEXT NOT NULL,
                email TEXT NOT NULL,
                name TEXT NOT NULL,
                country TEXT NOT NULL,
                item_count INTEGER NOT NULL,
                total_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                payment_id TEXT NOT NULL,
                refunded_cents INTEGER NOT NULL,
                placed_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                discount_id TEXT,
                discount_code TEXT,
                discount_cents INTEGER NOT NULL,
                fulfilled_at INTEGER,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX orders_orders_placed ON orders_orders (tenant_id, placed_at);
        CREATE INDEX orders_orders_status ON orders_orders (tenant_id, status, placed_at);
        CREATE INDEX orders_orders_discount ON orders_orders (tenant_id, discount_id)
                WHERE discount_id IS NOT NULL;
        CREATE INDEX orders_orders_email ON orders_orders (tenant_id, email, placed_at);
        CREATE INDEX orders_orders_fulfilled ON orders_orders (fulfilled_at)
                WHERE fulfilled_at IS NOT NULL;
        CREATE TABLE orders_carts (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                order_id TEXT,
                status TEXT NOT NULL,
                line_count INTEGER NOT NULL,
                item_count INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX orders_carts_open ON orders_carts (tenant_id, updated_at)
                WHERE order_id IS NULL;",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &["orders_orders", "orders_carts"];
