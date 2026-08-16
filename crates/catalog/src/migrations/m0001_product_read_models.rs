use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001ProductReadModels;

// One table per query shape rather than one table with a status filter: the
// storefront grid, the product page and the admin table want different columns
// and different rows, and denormalising keeps each query a single-table scan.
// `created_at` is epoch milliseconds so lists stay ordered when several
// products are imported within the same second.
sqlite_migration!(
    M0001ProductReadModels,
    "catalog",
    "m0001_product_read_models",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE store_product_list (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                price_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                image_url TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE store_product_list"
        ),
        (
            "CREATE TABLE store_product_detail (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                price_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                image_url TEXT NOT NULL,
                supplier_id TEXT NOT NULL,
                supplier_product_ref TEXT NOT NULL,
                published INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE store_product_detail"
        ),
        (
            "CREATE TABLE admin_product_list (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                price_cents INTEGER NOT NULL,
                currency TEXT NOT NULL,
                supplier_id TEXT NOT NULL,
                supplier_product_ref TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            "DROP TABLE admin_product_list"
        )
    ]
);
