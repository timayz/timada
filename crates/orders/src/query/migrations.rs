//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "orders";

/// The read models of this context, one table each (see the columns). An
/// order's row carries the whole order as its page shows it (`state`, the
/// [`OrderRecord`](crate::OrderRecord) JSON); a cart's row, the payments of
/// it the provider would not give back (`refused_returns`); and
/// `orders_order_mails` which mails the shopper was sent about an order.
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
                supplier TEXT,
                payment_state TEXT NOT NULL,
                capture_before INTEGER,
                capture_requested_at INTEGER,
                cancel_requested_at INTEGER,
                refund_requested_at INTEGER,
                paid_at INTEGER,
                hold_reminder_at INTEGER,
                state TEXT NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX orders_orders_placed ON orders_orders (tenant_id, placed_at);
        CREATE INDEX orders_orders_hold_reminder ON orders_orders (hold_reminder_at)
                WHERE hold_reminder_at IS NOT NULL;
        CREATE INDEX orders_orders_payment_due ON orders_orders (tenant_id, placed_at)
                WHERE status = 'authorized' OR cancel_requested_at IS NOT NULL
                    OR refund_requested_at IS NOT NULL;
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
                return_requested_at INTEGER,
                unanswered_since INTEGER,
                refused_returns TEXT NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX orders_carts_open ON orders_carts (tenant_id, updated_at)
                WHERE order_id IS NULL;
        CREATE INDEX orders_carts_returning ON orders_carts (return_requested_at)
                WHERE return_requested_at IS NOT NULL;
        CREATE INDEX orders_carts_unanswered ON orders_carts (unanswered_since)
                WHERE unanswered_since IS NOT NULL;
        CREATE TABLE orders_order_products (
                tenant_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                PRIMARY KEY (tenant_id, product_id, order_id)
            );
        CREATE TABLE orders_order_mails (
                tenant_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                mail TEXT NOT NULL,
                mailed_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, order_id, mail)
            );",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscriptions fill them again.
pub const TABLES: &[&str] = &[
    "orders_orders",
    "orders_carts",
    "orders_order_products",
    "orders_order_mails",
];
