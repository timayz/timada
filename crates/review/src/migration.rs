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

pub struct M0002ReviewList;

sqlite_migration!(
    M0002ReviewList,
    "review",
    "m0002_review_list",
    vec_box![M0001ReviewProductReview],
    vec_box![
        (
            "CREATE TABLE review_list (
                review_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                verified_purchase INTEGER NOT NULL,
                rating INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                status TEXT NOT NULL,
                rejection_reason TEXT,
                submitted_at INTEGER NOT NULL
            )",
            "DROP TABLE review_list"
        ),
        (
            "CREATE INDEX review_list_product ON review_list (product_id, status, submitted_at)",
            "DROP INDEX review_list_product"
        ),
        (
            "CREATE INDEX review_list_status ON review_list (status, submitted_at)",
            "DROP INDEX review_list_status"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![M0001ReviewProductReview, M0002ReviewList]
}
