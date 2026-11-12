//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "orders";

/// The read models of this context, one table each (see the columns). An
/// order's row carries the whole order as its page shows it (`state`, the
/// [`OrderRecord`](crate::OrderRecord) JSON); a cart's row, the payments of
/// it the provider would not give back (`refused_returns`); and
/// `orders_order_mails` where each mail to the shopper about an order stands.
/// An order's `search_text` is its reference, e-mail address and name as a
/// search finds them (`timada_core::db::tenant_search_key`), written once:
/// none of the three changes after the order is placed. `orders_tallies`
/// holds how many orders and open carts each tenant has, for its
/// home (see `ORDERS` and `OPEN_CARTS`).
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
                supplier TEXT,
                payment_state TEXT NOT NULL,
                capture_before INTEGER,
                capture_requested_at INTEGER,
                cancel_requested_at INTEGER,
                refund_requested_at INTEGER,
                paid_at INTEGER,
                hold_reminder_at INTEGER,
                state TEXT NOT NULL,
                search_text TEXT NOT NULL,
                PRIMARY KEY (tenant_id, id)
            );
        CREATE INDEX orders_orders_placed ON orders_orders (tenant_id, placed_at, id);
        CREATE INDEX orders_orders_hold_reminder ON orders_orders (hold_reminder_at)
                WHERE hold_reminder_at IS NOT NULL;
        CREATE INDEX orders_orders_payment_due ON orders_orders (tenant_id, placed_at)
                WHERE status = 'authorized' OR cancel_requested_at IS NOT NULL
                    OR refund_requested_at IS NOT NULL;
        CREATE INDEX orders_orders_status ON orders_orders (tenant_id, status, placed_at, id);
        CREATE INDEX orders_orders_search ON orders_orders USING fts(search_text)
                WITH (tokenizer = 'ngram', min_gram = 3, max_gram = 3);
        CREATE INDEX orders_orders_discount ON orders_orders (tenant_id, discount_id)
                WHERE discount_id IS NOT NULL;
        CREATE INDEX orders_orders_email ON orders_orders (tenant_id, email, placed_at);
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
        CREATE INDEX orders_carts_paying ON orders_carts (tenant_id, updated_at, id)
                WHERE status = 'paying';
        CREATE INDEX orders_carts_idle ON orders_carts (updated_at, id)
                WHERE status = 'open';
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
                status TEXT NOT NULL,
                attempts INTEGER NOT NULL,
                reason TEXT NOT NULL,
                sent_at INTEGER,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, order_id, mail)
            );
        CREATE TABLE orders_tallies (
                tenant_id TEXT NOT NULL,
                name TEXT NOT NULL,
                n INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, name)
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
    "orders_tallies",
];
