use anyhow::Context as _;
use evento::migrator::Migrate as _;
use sqlx::SqlitePool;
use sqlx_migrator::{Info as _, Migration, vec_box};

/// The app name under which these migrations are recorded (the macro needs
/// the literal, so it is repeated there).
pub const MIGRATION_APP: &str = "storefront";

/// The local tables of the library. [`Store::connect`](crate::Store::connect)
/// applies them with evento's own; a host running its own migrator can add
/// them to it instead.
pub fn migrations() -> Vec<Box<dyn Migration<sqlx::Sqlite>>> {
    vec_box![
        Products0001,
        Categories0002,
        ProductImages0003,
        ProductVariants0004,
        ProductSearch0005,
        Reviews0006,
        CategoryMenu0007
    ]
}

/// Applies evento's tables and the library's own to `pool`.
/// [`Store::connect`](crate::Store::connect) runs it; a host can also run it
/// ahead of time, without reaching the store.
pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut conn = pool.acquire().await?;
    let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().map_err(anyhow::Error::from)?;
    migrator
        .add_migrations(migrations())
        .map_err(anyhow::Error::from)?;
    migrator
        .run(&mut *conn, &evento::migrator::Plan::apply_all())
        .await
        .context("cannot prepare the local database")?;
    Ok(())
}

pub struct Products0001;
sqlx_migrator::sqlite_migration!(
    Products0001,
    "storefront",
    "0001_products",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE storefront_products (
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                amount_cents INTEGER,
                currency TEXT,
                status TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            "DROP TABLE storefront_products"
        ),
        (
            "CREATE UNIQUE INDEX storefront_products_slug ON storefront_products (slug)
                WHERE status <> 'archived'",
            "DROP INDEX storefront_products_slug"
        ),
        (
            "CREATE INDEX storefront_products_published ON storefront_products (status, updated_at)",
            "DROP INDEX storefront_products_published"
        ),
    ]
);

pub struct Categories0002;
sqlx_migrator::sqlite_migration!(
    Categories0002,
    "storefront",
    "0002_categories",
    vec_box![("storefront", "0001_products")],
    vec_box![
        (
            "CREATE TABLE storefront_categories (
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                parent_id TEXT,
                position INTEGER NOT NULL,
                archived INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                product_count INTEGER NOT NULL DEFAULT 0,
                subtree_count INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE storefront_categories"
        ),
        (
            "CREATE UNIQUE INDEX storefront_categories_slug ON storefront_categories (slug)
                WHERE archived = 0",
            "DROP INDEX storefront_categories_slug"
        ),
        (
            "CREATE TABLE storefront_product_categories (
                product_id TEXT NOT NULL,
                category_id TEXT NOT NULL,
                PRIMARY KEY (product_id, category_id)
            )",
            "DROP TABLE storefront_product_categories"
        ),
        (
            "CREATE INDEX storefront_product_categories_category
                ON storefront_product_categories (category_id)",
            "DROP INDEX storefront_product_categories_category"
        ),
        (
            "CREATE TABLE storefront_category_products (
                category_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                PRIMARY KEY (category_id, product_id)
            )",
            "DROP TABLE storefront_category_products"
        ),
    ]
);

pub struct ProductImages0003;
sqlx_migrator::sqlite_migration!(
    ProductImages0003,
    "storefront",
    "0003_product_images",
    vec_box![("storefront", "0002_categories")],
    vec_box![(
        // The gallery as JSON: a page shows a product whole, and the row is
        // rewritten whole from the store, so a table of its own would buy
        // nothing.
        "ALTER TABLE storefront_products ADD COLUMN images_json TEXT NOT NULL DEFAULT '[]'",
        "ALTER TABLE storefront_products DROP COLUMN images_json"
    )]
);

pub struct ProductVariants0004;
sqlx_migrator::sqlite_migration!(
    ProductVariants0004,
    "storefront",
    "0004_product_variants",
    vec_box![("storefront", "0003_product_images")],
    vec_box![
        (
            "ALTER TABLE storefront_products ADD COLUMN options_json TEXT NOT NULL DEFAULT '[]'",
            "ALTER TABLE storefront_products DROP COLUMN options_json"
        ),
        (
            "ALTER TABLE storefront_products ADD COLUMN variants_json TEXT NOT NULL DEFAULT '[]'",
            "ALTER TABLE storefront_products DROP COLUMN variants_json"
        ),
        (
            "ALTER TABLE storefront_products ADD COLUMN attributes_json TEXT NOT NULL DEFAULT '[]'",
            "ALTER TABLE storefront_products DROP COLUMN attributes_json"
        ),
        (
            // What a listing sorts and filters by when the price varies.
            "ALTER TABLE storefront_products ADD COLUMN min_amount_cents INTEGER",
            "ALTER TABLE storefront_products DROP COLUMN min_amount_cents"
        ),
        (
            "ALTER TABLE storefront_products ADD COLUMN max_amount_cents INTEGER",
            "ALTER TABLE storefront_products DROP COLUMN max_amount_cents"
        ),
        (
            "CREATE INDEX storefront_products_price
                ON storefront_products (status, min_amount_cents)",
            "DROP INDEX storefront_products_price"
        ),
        // A category's page reads its products straight off these, in the
        // order a shopper picked, without sorting.
        (
            "ALTER TABLE storefront_category_products
                ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE storefront_category_products DROP COLUMN updated_at"
        ),
        (
            "ALTER TABLE storefront_category_products ADD COLUMN min_amount_cents INTEGER",
            "ALTER TABLE storefront_category_products DROP COLUMN min_amount_cents"
        ),
        (
            "ALTER TABLE storefront_category_products ADD COLUMN max_amount_cents INTEGER",
            "ALTER TABLE storefront_category_products DROP COLUMN max_amount_cents"
        ),
        (
            "CREATE INDEX storefront_category_products_newest ON storefront_category_products
                (category_id, updated_at DESC, product_id DESC)",
            "DROP INDEX storefront_category_products_newest"
        ),
        (
            "CREATE INDEX storefront_category_products_price_asc ON storefront_category_products
                (category_id, min_amount_cents IS NULL, min_amount_cents, product_id DESC)",
            "DROP INDEX storefront_category_products_price_asc"
        ),
        (
            "CREATE INDEX storefront_category_products_price_desc ON storefront_category_products
                (category_id, max_amount_cents IS NULL, max_amount_cents DESC, product_id DESC)",
            "DROP INDEX storefront_category_products_price_desc"
        ),
    ]
);

pub struct ProductSearch0005;
sqlx_migrator::sqlite_migration!(
    ProductSearch0005,
    "storefront",
    "0005_product_search",
    vec_box![("storefront", "0004_product_variants")],
    vec_box![
        (
            // Cut in runs of three characters, so a search matches anywhere
            // in a word as `LIKE '%q%'` did, ignoring case and accents. The
            // rowid is the product row's.
            "CREATE VIRTUAL TABLE storefront_products_fts USING fts5(
                slug, name, tokenize = 'trigram remove_diacritics 1'
            )",
            "DROP TABLE storefront_products_fts"
        ),
        (
            "INSERT INTO storefront_products_fts (rowid, slug, name)
                SELECT rowid, slug, name FROM storefront_products",
            "DELETE FROM storefront_products_fts"
        ),
    ]
);

pub struct Reviews0006;
sqlx_migrator::sqlite_migration!(
    Reviews0006,
    "storefront",
    "0006_reviews",
    vec_box![("storefront", "0005_product_search")],
    vec_box![
        (
            // Published reviews only: one that is rejected leaves.
            "CREATE TABLE storefront_reviews (
                id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                rating INTEGER NOT NULL,
                title TEXT,
                body TEXT NOT NULL,
                author_name TEXT NOT NULL,
                variant_label TEXT,
                photos_json TEXT NOT NULL,
                published_at INTEGER NOT NULL
            )",
            "DROP TABLE storefront_reviews"
        ),
        (
            "CREATE INDEX storefront_reviews_product ON storefront_reviews
                (product_id, published_at DESC, id DESC)",
            "DROP INDEX storefront_reviews_product"
        ),
        // A product's published reviews, counted, and their average times a
        // hundred (`435` is 4.35 stars), so a listing sorts by them.
        (
            "ALTER TABLE storefront_products
                ADD COLUMN review_count INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE storefront_products DROP COLUMN review_count"
        ),
        (
            "ALTER TABLE storefront_products
                ADD COLUMN rating_x100 INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE storefront_products DROP COLUMN rating_x100"
        ),
        (
            "ALTER TABLE storefront_category_products
                ADD COLUMN review_count INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE storefront_category_products DROP COLUMN review_count"
        ),
        (
            "ALTER TABLE storefront_category_products
                ADD COLUMN rating_x100 INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE storefront_category_products DROP COLUMN rating_x100"
        ),
        (
            "CREATE INDEX storefront_category_products_top_rated ON storefront_category_products
                (category_id, review_count = 0, rating_x100 DESC, review_count DESC,
                 product_id DESC)",
            "DROP INDEX storefront_category_products_top_rated"
        ),
    ]
);

pub struct CategoryMenu0007;
sqlx_migrator::sqlite_migration!(
    CategoryMenu0007,
    "storefront",
    "0007_category_menu",
    vec_box![("storefront", "0006_reviews")],
    vec_box![(
        "ALTER TABLE storefront_categories ADD COLUMN in_menu INTEGER NOT NULL DEFAULT 1",
        "ALTER TABLE storefront_categories DROP COLUMN in_menu"
    )]
);
