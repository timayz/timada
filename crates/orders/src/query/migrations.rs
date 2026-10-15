use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "orders";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![
        Orders0001,
        OrderDiscounts0002,
        OrdersByEmail0003,
        FulfilledAt0004
    ]
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

pub struct OrderDiscounts0002;
sqlx_migrator::sqlite_migration!(
    OrderDiscounts0002,
    "orders",
    "0002_order_discounts",
    vec_box![("orders", "0001_orders_and_carts")],
    vec_box![
        (
            "ALTER TABLE orders_orders ADD COLUMN discount_id TEXT",
            "ALTER TABLE orders_orders DROP COLUMN discount_id"
        ),
        (
            "ALTER TABLE orders_orders ADD COLUMN discount_code TEXT",
            "ALTER TABLE orders_orders DROP COLUMN discount_code"
        ),
        (
            "ALTER TABLE orders_orders ADD COLUMN discount_cents INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE orders_orders DROP COLUMN discount_cents"
        ),
        (
            "CREATE INDEX orders_orders_discount ON orders_orders (tenant_id, discount_id)
                WHERE discount_id IS NOT NULL",
            "DROP INDEX orders_orders_discount"
        ),
    ]
);

/// A shopper's orders are the ones placed with their address.
pub struct OrdersByEmail0003;
sqlx_migrator::sqlite_migration!(
    OrdersByEmail0003,
    "orders",
    "0003_orders_by_email",
    vec_box![("orders", "0002_order_discounts")],
    vec_box![(
        "CREATE INDEX orders_orders_email ON orders_orders (tenant_id, email, placed_at)",
        "DROP INDEX orders_orders_email"
    )]
);

/// When an order left, so its buyer can be asked for a review some days
/// later. Orders fulfilled before this column existed keep `NULL`.
pub struct FulfilledAt0004;
sqlx_migrator::sqlite_migration!(
    FulfilledAt0004,
    "orders",
    "0004_fulfilled_at",
    vec_box![("orders", "0003_orders_by_email")],
    vec_box![
        (
            "ALTER TABLE orders_orders ADD COLUMN fulfilled_at INTEGER",
            "ALTER TABLE orders_orders DROP COLUMN fulfilled_at"
        ),
        (
            "CREATE INDEX orders_orders_fulfilled ON orders_orders (fulfilled_at)
                WHERE fulfilled_at IS NOT NULL",
            "DROP INDEX orders_orders_fulfilled"
        ),
    ]
);
