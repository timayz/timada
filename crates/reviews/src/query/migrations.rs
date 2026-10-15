use sqlx_migrator::{Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "reviews";

pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![Reviews0001]
}

pub struct Reviews0001;
sqlx_migrator::sqlite_migration!(
    Reviews0001,
    "reviews",
    "0001_reviews",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE reviews_reviews (
                tenant_id TEXT NOT NULL,
                id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                order_reference TEXT NOT NULL,
                product_id TEXT NOT NULL,
                product_name TEXT NOT NULL,
                variant_label TEXT,
                rating INTEGER NOT NULL,
                title TEXT,
                body TEXT NOT NULL,
                author_name TEXT NOT NULL,
                photo_count INTEGER NOT NULL,
                status TEXT NOT NULL,
                submitted_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )",
            "DROP TABLE reviews_reviews"
        ),
        (
            "CREATE INDEX reviews_reviews_status ON reviews_reviews
                (tenant_id, status, submitted_at DESC)",
            "DROP INDEX reviews_reviews_status"
        ),
        (
            "CREATE INDEX reviews_reviews_product ON reviews_reviews
                (tenant_id, product_id, submitted_at DESC)",
            "DROP INDEX reviews_reviews_product"
        ),
        (
            "CREATE TABLE reviews_product_ratings (
                tenant_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                review_count INTEGER NOT NULL,
                rating_sum INTEGER NOT NULL,
                PRIMARY KEY (tenant_id, product_id)
            )",
            "DROP TABLE reviews_product_ratings"
        ),
    ]
);
