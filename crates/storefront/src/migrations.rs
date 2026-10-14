use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "storefront";

/// The local tables of the library. [`Store::connect`](crate::Store::connect)
/// applies them with evento's own; a host running its own migrator can add
/// them to it instead.
pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Products0001, Categories0002, ProductImages0003]
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

pub struct Categories0002;
sqlx_migrator::sqlite_migration!(
    Categories0002,
    "storefront",
    "0002_categories",
    vec_box![("storefront", "0001_products")],
    vec_box![
        (
            "CREATE TABLE storefront_categories (
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                parent_id TEXT,
                position INTEGER NOT NULL,
                archived INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                product_count INTEGER NOT NULL DEFAULT 0,
                subtree_count INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE storefront_categories"
        ),
        (
            "CREATE UNIQUE INDEX storefront_categories_slug ON storefront_categories (slug)
                WHERE archived = 0",
            "DROP INDEX storefront_categories_slug"
        ),
        (
            "CREATE TABLE storefront_product_categories (
                product_id TEXT NOT NULL,
                category_id TEXT NOT NULL,
                PRIMARY KEY (product_id, category_id)
            )",
            "DROP TABLE storefront_product_categories"
        ),
        (
            "CREATE INDEX storefront_product_categories_category
                ON storefront_product_categories (category_id)",
            "DROP INDEX storefront_product_categories_category"
        ),
        (
            "CREATE TABLE storefront_category_products (
                category_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                PRIMARY KEY (category_id, product_id)
            )",
            "DROP TABLE storefront_category_products"
        ),
    ]
);

pub struct ProductImages0003;
sqlx_migrator::sqlite_migration!(
    ProductImages0003,
    "storefront",
    "0003_product_images",
    vec_box![("storefront", "0002_categories")],
    vec_box![(
        // The gallery as JSON: a page shows a product whole, and the row is
        // rewritten whole from the store, so a table of its own would buy
        // nothing.
        "ALTER TABLE storefront_products ADD COLUMN images_json TEXT NOT NULL DEFAULT '[]'",
        "ALTER TABLE storefront_products DROP COLUMN images_json"
    )]
);
