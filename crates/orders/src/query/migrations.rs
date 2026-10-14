use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "orders";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Orders0001]
}

pub struct Orders0001;
sqlx_migrator::sqlite_migration!(
    Orders0001,
    "orders",
    "0001_orders_and_carts",
    vec_box![],
    vec_box![
        (
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
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE orders_orders"
        ),
        (
            "CREATE INDEX orders_orders_placed ON orders_orders (tenant_id, placed_at)",
            "DROP INDEX orders_orders_placed"
        ),
        (
            "CREATE INDEX orders_orders_status ON orders_orders (tenant_id, status, placed_at)",
            "DROP INDEX orders_orders_status"
        ),
        (
            "CREATE TABLE orders_carts (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                order_id TEXT,
                status TEXT NOT NULL,
                line_count INTEGER NOT NULL,
                item_count INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE orders_carts"
        ),
        (
            "CREATE INDEX orders_carts_open ON orders_carts (tenant_id, updated_at)
                WHERE order_id IS NULL",
            "DROP INDEX orders_carts_open"
        ),
    ]
);
