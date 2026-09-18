use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001ReviewProductReview;

sqlite_migration!(
    M0001ReviewProductReview,
    "review",
    "m0001_review_product_review",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE review_product_review (
                review_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                rating INTEGER NOT NULL,
                published INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE review_product_review"
        ),
        (
            "CREATE INDEX review_product_review_product
                ON review_product_review (product_id, published)",
            "DROP INDEX review_product_review_product"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001ReviewProductReview]
}
