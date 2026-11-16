//! Schema of the read model: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "pages";

/// `pages`: one row per page of a tenant; `articles`: one per article of
/// its blog.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_pages",
        "CREATE TABLE pages (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            slug TEXT NOT NULL,
            title TEXT NOT NULL,
            published INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, id)
        );
        -- One row per article of a tenant's blog. `listed_at` is what the
        -- list sorts by: the day the article says it came out, or when it
        -- was written while it never did.
        CREATE TABLE articles (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            slug TEXT NOT NULL,
            title TEXT NOT NULL,
            published INTEGER NOT NULL,
            published_at INTEGER NOT NULL,
            listed_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, id)
        );
        CREATE INDEX articles_listed ON articles (tenant_id, listed_at DESC, id DESC);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &["pages", "articles"];
