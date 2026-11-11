//! Schema of the read models: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "catalog";

/// The catalog read models: products with what a search finds them by and
/// what a cart is priced with (their combinations, the shop's delivery
/// time), the categories with their counts, which product is in which
/// category, the stock of each thing sold, and how many products each
/// tenant has (`catalog_tallies`, one row per count, see `PRODUCTS`).
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
        // anywhere in a word. `slug_search` starts with the tenant's token
        // (`timada_core::db::tenant_search_key`), which every search must
        // match too: the index then answers with the tenant's own products
        // only, not with every shop's. There is no description: no page
        // reads it from here, and a list sorting rows carried it along.
        // `tax_category` and the two `delivery_*_days` (`NULL`: the shop's
        // default) are what a cart is priced with, with `catalog_variants`:
        // one row per combination, `on_sale` for those a shopper can buy,
        // `label` the values as the shop shows them (`Sable / Large`).
        // `catalog_delivery_default` is the shop's delivery time, one row per
        // tenant that set one.
        "CREATE TABLE catalog_products (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            slug TEXT NOT NULL,
            name TEXT NOT NULL,
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
            tax_category TEXT NOT NULL,
            delivery_min_days INTEGER,
            delivery_max_days INTEGER,
            stock_state TEXT NOT NULL DEFAULT 'untracked',
            slug_search TEXT NOT NULL,
            name_search TEXT NOT NULL,
            PRIMARY KEY (tenant_id, id)
        );
        CREATE TABLE catalog_variants (
            tenant_id TEXT NOT NULL,
            product_id TEXT NOT NULL,
            id TEXT NOT NULL,
            position INTEGER NOT NULL,
            sku TEXT,
            label TEXT,
            amount_cents INTEGER NOT NULL,
            currency TEXT NOT NULL,
            on_sale INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, product_id, id)
        );
        CREATE TABLE catalog_delivery_default (
            tenant_id TEXT NOT NULL,
            min_days INTEGER,
            max_days INTEGER,
            PRIMARY KEY (tenant_id)
        );
        CREATE UNIQUE INDEX catalog_products_slug ON catalog_products (tenant_id, slug)
            WHERE status <> 'archived';
        CREATE INDEX catalog_products_changed
            ON catalog_products (tenant_id, updated_at, id);
        CREATE INDEX catalog_products_status_changed
            ON catalog_products (tenant_id, status, updated_at, id);
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
        );
        CREATE TABLE catalog_tallies (
            tenant_id TEXT NOT NULL,
            name TEXT NOT NULL,
            n INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, name)
        );",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &[
    "catalog_products",
    "catalog_variants",
    "catalog_delivery_default",
    "catalog_categories",
    "catalog_product_categories",
    "catalog_stock",
    "catalog_tallies",
];
