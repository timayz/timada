//! Schema of the read model, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "google";

/// `google_connections`: one row per shop whose connection reads Search
/// Console and has a site chosen: where the daily pass looks for shops.
/// `google_search_watch`: where each shop's pass stands, and from when the
/// next is due (`next_at`); `site` is the one it last looked at, so a shop
/// whose site changed is due at once.
/// `google_search_days`: what a site did on a day. `google_search_queries`
/// and `google_search_pages`: the most clicked queries and pages of a day.
/// A position is in thousandths. The three are keyed by shop, site and day,
/// so a period is one range of the key, and a site's rows are left behind,
/// to age out, when the shop chooses another.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_search",
        "CREATE TABLE google_connections (
            tenant_id TEXT NOT NULL PRIMARY KEY,
            site TEXT NOT NULL,
            lapsed INTEGER NOT NULL
        );
        CREATE TABLE google_search_watch (
            tenant_id TEXT NOT NULL PRIMARY KEY,
            site TEXT NOT NULL,
            next_at INTEGER NOT NULL
        );
        CREATE TABLE google_search_days (
            tenant_id TEXT NOT NULL,
            site TEXT NOT NULL,
            day TEXT NOT NULL,
            clicks INTEGER NOT NULL,
            impressions INTEGER NOT NULL,
            position_milli INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, site, day)
        );
        CREATE TABLE google_search_queries (
            tenant_id TEXT NOT NULL,
            site TEXT NOT NULL,
            day TEXT NOT NULL,
            query TEXT NOT NULL,
            clicks INTEGER NOT NULL,
            impressions INTEGER NOT NULL,
            position_milli INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, site, day, query)
        );
        CREATE TABLE google_search_pages (
            tenant_id TEXT NOT NULL,
            site TEXT NOT NULL,
            day TEXT NOT NULL,
            page TEXT NOT NULL,
            clicks INTEGER NOT NULL,
            impressions INTEGER NOT NULL,
            position_milli INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, site, day, page)
        );",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &[
    "google_connections",
    "google_search_watch",
    "google_search_days",
    "google_search_queries",
    "google_search_pages",
];
