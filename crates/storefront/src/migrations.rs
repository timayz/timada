use evento::turso::migrate::{Migration, Migrations};
use timada_core::db::Db;

/// The app name under which these migrations are recorded in the local
/// database (evento's own are the app `evento`).
pub const MIGRATION_APP: &str = "storefront";

/// The local tables of the library, applied by [`migrate`] next to evento's
/// own ([`Store::connect`](crate::Store::connect) runs it); a host running its
/// own migrations can apply them itself.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_storefront",
        // One row per product copied from the store, rewritten whole when
        // the product changes. The gallery, options, variants and
        // attributes are JSON: a page shows a product whole, so tables of
        // their own would buy nothing. `min_amount_cents` and
        // `max_amount_cents` are what a listing sorts and filters by when
        // the price varies. `review_count` and `rating_x100` (`435` is
        // 4.35 stars) sum the published reviews; `stock_json` and
        // `in_stock` are what a page shows of the shelf. Those four are
        // worked out after the row is written, hence their defaults.
        // `slug_search` and `name_search` are what a search finds a product
        // by: the slug and the name lowercased and stripped of accents,
        // under an index cut in runs of three characters so a search matches
        // anywhere in a word as `LIKE '%q%'` would.
        "CREATE TABLE storefront_products (
            id TEXT PRIMARY KEY,
            slug TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            amount_cents INTEGER,
            currency TEXT,
            status TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            images_json TEXT NOT NULL,
            options_json TEXT NOT NULL,
            variants_json TEXT NOT NULL,
            attributes_json TEXT NOT NULL,
            min_amount_cents INTEGER,
            max_amount_cents INTEGER,
            review_count INTEGER NOT NULL DEFAULT 0,
            rating_x100 INTEGER NOT NULL DEFAULT 0,
            stock_json TEXT NOT NULL DEFAULT '{}',
            in_stock INTEGER NOT NULL DEFAULT 1,
            slug_search TEXT NOT NULL,
            name_search TEXT NOT NULL
        );
        CREATE UNIQUE INDEX storefront_products_slug ON storefront_products (slug)
            WHERE status <> 'archived';
        CREATE INDEX storefront_products_published ON storefront_products (status, updated_at);
        CREATE INDEX storefront_products_price
            ON storefront_products (status, min_amount_cents);
        CREATE INDEX storefront_products_fts ON storefront_products
            USING fts(slug_search, name_search)
            WITH (tokenizer = 'ngram', min_gram = 3, max_gram = 3);
        -- One row per declared value of a product's options, keyed the way a
        -- filter compares (see `facet_key`): what \"Colour: Rose\" narrows a
        -- listing by, and what the colours of a listing are counted from.
        CREATE TABLE storefront_product_options (
            product_id TEXT NOT NULL,
            name_key TEXT NOT NULL,
            value_key TEXT NOT NULL,
            name TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (product_id, name_key, value_key)
        );
        CREATE INDEX storefront_product_options_value ON storefront_product_options
            (name_key, value_key, product_id);
        -- The counts are refreshed from the product rows, after the
        -- category's own row is written.
        CREATE TABLE storefront_categories (
            id TEXT PRIMARY KEY,
            slug TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            body TEXT NOT NULL,
            parent_id TEXT,
            position INTEGER NOT NULL,
            archived INTEGER NOT NULL,
            in_menu INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            product_count INTEGER NOT NULL DEFAULT 0,
            subtree_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE UNIQUE INDEX storefront_categories_slug ON storefront_categories (slug)
            WHERE archived = 0;
        CREATE TABLE storefront_product_categories (
            product_id TEXT NOT NULL,
            category_id TEXT NOT NULL,
            PRIMARY KEY (product_id, category_id)
        );
        CREATE INDEX storefront_product_categories_category
            ON storefront_product_categories (category_id);
        -- A category's page reads its published products straight off these
        -- rows, in the order a shopper picked, without sorting: the sort keys
        -- are copied from the product row.
        CREATE TABLE storefront_category_products (
            category_id TEXT NOT NULL,
            product_id TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            min_amount_cents INTEGER,
            max_amount_cents INTEGER,
            review_count INTEGER NOT NULL,
            rating_x100 INTEGER NOT NULL,
            PRIMARY KEY (category_id, product_id)
        );
        CREATE INDEX storefront_category_products_newest ON storefront_category_products
            (category_id, updated_at DESC, product_id DESC);
        CREATE INDEX storefront_category_products_price_asc ON storefront_category_products
            (category_id, min_amount_cents IS NULL, min_amount_cents, product_id DESC);
        CREATE INDEX storefront_category_products_price_desc ON storefront_category_products
            (category_id, max_amount_cents IS NULL, max_amount_cents DESC, product_id DESC);
        CREATE INDEX storefront_category_products_top_rated ON storefront_category_products
            (category_id, review_count = 0, rating_x100 DESC, review_count DESC,
             product_id DESC);
        -- Published reviews only: one that is rejected leaves. `reply` and
        -- `replied_at` hold the merchant's public answer, while there is one.
        CREATE TABLE storefront_reviews (
            id TEXT PRIMARY KEY,
            product_id TEXT NOT NULL,
            rating INTEGER NOT NULL,
            title TEXT,
            body TEXT NOT NULL,
            author_name TEXT NOT NULL,
            variant_label TEXT,
            photos_json TEXT NOT NULL,
            published_at INTEGER NOT NULL,
            reply TEXT,
            replied_at INTEGER
        );
        CREATE INDEX storefront_reviews_product ON storefront_reviews
            (product_id, published_at DESC, id DESC);
        -- What is on the shelf of each thing the shop counts (`variant_id` ''
        -- for a product without variants). The product row carries what a
        -- page shows of it, worked out when the stock or the product changes.
        CREATE TABLE storefront_stock (
            product_id TEXT NOT NULL,
            variant_id TEXT NOT NULL DEFAULT '',
            tracked INTEGER NOT NULL,
            on_hand INTEGER NOT NULL,
            PRIMARY KEY (product_id, variant_id)
        );
        CREATE TABLE storefront_pages (
            id TEXT PRIMARY KEY,
            slug TEXT NOT NULL,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            text TEXT NOT NULL,
            meta_title TEXT NOT NULL,
            meta_description TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX storefront_pages_slug ON storefront_pages (slug);",
    )],
);

/// The tables [`MIGRATIONS`] create: what a copy of the catalog is made of.
pub const TABLES: &[&str] = &[
    "storefront_products",
    "storefront_product_options",
    "storefront_categories",
    "storefront_product_categories",
    "storefront_category_products",
    "storefront_reviews",
    "storefront_stock",
    "storefront_pages",
];

/// Applies the library's pending migrations to the local database (evento's
/// own tables are there since it was opened).
/// [`Store::connect`](crate::Store::connect) runs it; a host can also run it
/// ahead of time, without reaching the store.
pub async fn migrate(db: &Db) -> anyhow::Result<()> {
    let mut conn = db.connect()?;
    MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(applied, err)| {
            anyhow::anyhow!("cannot prepare the local database: {err} ({applied} applied)")
        })?;
    Ok(())
}
