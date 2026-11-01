//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "catalog";

/// The catalog read models: products with what a search finds them by, the
/// categories with their counts, which product is in which category, and
/// the stock of each thing sold.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_products",
        // One row per product of a tenant. `thumbnail` is the first picture's
        // file name, for a listing's thumbnail. When a product varies,
        // `min_amount_cents` and `max_amount_cents` are the cheapest and the
        // dearest a shopper may pay. `stock_state` is worked out from the
        // stock rows after the row is written, hence its default.
        // `slug_search` and `name_search` are what a search finds a product
        // by: the slug and the name lowercased and stripped of accents, under
        // an index cut in runs of three characters so a search matches
        // anywhere in a word; the tenant narrows it to its own products.
        "CREATE TABLE catalog_products (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            slug TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            amount_cents INTEGER,
            currency TEXT,
            status TEXT NOT NULL,
            created_by TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            thumbnail TEXT,
            image_count INTEGER NOT NULL,
            min_amount_cents INTEGER,
            max_amount_cents INTEGER,
            variant_count INTEGER NOT NULL,
            stock_state TEXT NOT NULL DEFAULT 'untracked',
            slug_search TEXT NOT NULL,
            name_search TEXT NOT NULL,
            PRIMARY KEY (tenant_id, id)
        );
        CREATE UNIQUE INDEX catalog_products_slug ON catalog_products (tenant_id, slug)
            WHERE status <> 'archived';
        CREATE INDEX catalog_products_name ON catalog_products (tenant_id, name);
        CREATE INDEX catalog_products_fts ON catalog_products
            USING fts(slug_search, name_search)
            WITH (tokenizer = 'ngram', min_gram = 3, max_gram = 3);
        CREATE TABLE catalog_categories (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            slug TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            parent_id TEXT,
            position INTEGER NOT NULL,
            archived INTEGER NOT NULL,
            in_menu INTEGER NOT NULL,
            created_by TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            product_count INTEGER NOT NULL DEFAULT 0,
            subtree_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (tenant_id, id)
        );
        CREATE UNIQUE INDEX catalog_categories_slug ON catalog_categories (tenant_id, slug)
            WHERE archived = 0;
        CREATE INDEX catalog_categories_parent
            ON catalog_categories (tenant_id, parent_id, position, name);
        CREATE TABLE catalog_product_categories (
            tenant_id TEXT NOT NULL,
            product_id TEXT NOT NULL,
            category_id TEXT NOT NULL,
            PRIMARY KEY (tenant_id, product_id, category_id)
        );
        CREATE INDEX catalog_product_categories_category
            ON catalog_product_categories (tenant_id, category_id);
        CREATE TABLE catalog_stock (
            tenant_id TEXT NOT NULL,
            product_id TEXT NOT NULL,
            variant_id TEXT NOT NULL DEFAULT '',
            tracked INTEGER NOT NULL,
            on_hand INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, product_id, variant_id)
        );",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &[
    "catalog_products",
    "catalog_categories",
    "catalog_product_categories",
    "catalog_stock",
];
