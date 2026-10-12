use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "storefront";

/// The local tables of the library. [`Store::connect`](crate::Store::connect)
/// applies them with evento's own; a host running its own migrator can add
/// them to it instead.
pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Products0001]
}

pub struct Products0001;
sqlx_migrator::sqlite_migration!(
    Products0001,
    "storefront",
    "0001_products",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE storefront_products (
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                amount_cents INTEGER,
                currency TEXT,
                status TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            "DROP TABLE storefront_products"
        ),
        (
            "CREATE UNIQUE INDEX storefront_products_slug ON storefront_products (slug)
                WHERE status <> 'archived'",
            "DROP INDEX storefront_products_slug"
        ),
        (
            "CREATE INDEX storefront_products_published ON storefront_products (status, updated_at)",
            "DROP INDEX storefront_products_published"
        ),
    ]
);
