//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "reviews";

/// `reviews_reviews` (one row per review of a tenant) and
/// `reviews_product_ratings` (a product's published reviews, counted).
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_reviews",
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
        );
        CREATE INDEX reviews_reviews_status ON reviews_reviews
            (tenant_id, status, submitted_at DESC);
        CREATE INDEX reviews_reviews_product ON reviews_reviews
            (tenant_id, product_id, submitted_at DESC);
        CREATE TABLE reviews_product_ratings (
            tenant_id TEXT NOT NULL,
            product_id TEXT NOT NULL,
            review_count INTEGER NOT NULL,
            rating_sum INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, product_id)
        );",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &["reviews_reviews", "reviews_product_ratings"];
