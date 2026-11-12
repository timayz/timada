//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "reviews";

/// `reviews_reviews` (one row per review of a tenant),
/// `reviews_product_ratings` (a product's published reviews, counted),
/// `reviews_requests` (the review request each fulfilled order owes: when it
/// falls due under the shop's policy, until it is mailed) and
/// `reviews_request_policies` (each shop's delay, `NULL` when it never asks;
/// no row is the default).
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
        );
        CREATE TABLE reviews_requests (
            tenant_id TEXT NOT NULL,
            order_id TEXT NOT NULL,
            fulfilled_at INTEGER,
            due_at INTEGER,
            mailed_at INTEGER,
            PRIMARY KEY (tenant_id, order_id)
        );
        CREATE INDEX reviews_requests_due ON reviews_requests (due_at, tenant_id, order_id)
            WHERE due_at IS NOT NULL;
        CREATE INDEX reviews_requests_fulfilled ON reviews_requests (tenant_id, fulfilled_at);
        CREATE TABLE reviews_request_policies (
            tenant_id TEXT NOT NULL PRIMARY KEY,
            delay_days INTEGER
        );",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &[
    "reviews_reviews",
    "reviews_product_ratings",
    "reviews_requests",
    "reviews_request_policies",
];
