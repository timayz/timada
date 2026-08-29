use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0002ProductPrices;

sqlite_migration!(
    M0002ProductPrices,
    "catalog",
    "m0002_product_prices",
    vec_box![("catalog", "m0001_product_read_models")],
    vec_box![(
        "CREATE TABLE store_product_prices (
            product_id TEXT NOT NULL,
            currency TEXT NOT NULL,
            amount_cents INTEGER NOT NULL,
            PRIMARY KEY (product_id, currency)
        )",
        "DROP TABLE store_product_prices"
    )]
);
