//! Schema of the pictures' tables, on its own so the integration tests that
//! include `db.rs` by path can include it too.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which the read model's migrations are recorded.
pub const MIGRATION_APP: &str = "media";

/// The cursor identity of the subscription that keeps the files' read model
/// and this node's wanted list in step with the catalog, the pages and the
/// reviews, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/media";

/// The cursor identity of the subscription that does the same for the mail
/// logos, in the platform store.
pub const LOGO_SUBSCRIPTION: &str = "timada-dashboard/mail-logos";

/// `media_files`: one row per rendition file the store knows, with what a
/// copy is checked against and what references it (`source`: `product`,
/// `page`, `review` or `logo`, and that thing's id). A read model of the
/// catalog's, the pages', the reviews' and the tenants' streams, the same
/// on every node.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_media_files",
        "CREATE TABLE media_files (
            tenant_id TEXT NOT NULL,
            image_id TEXT NOT NULL,
            rendition TEXT NOT NULL,
            size INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            source TEXT NOT NULL,
            source_id TEXT NOT NULL,
            added_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, image_id, rendition)
        );
        CREATE INDEX media_files_source ON media_files (tenant_id, source, source_id);",
    )],
);

/// The tables the migrations create: what a node drops and creates again
/// before copying its store from a peer, so the subscriptions fill them
/// again.
pub const TABLES: &[&str] = &["media_files"];

/// The app name under which this node's own table is recorded.
pub const NODE_MIGRATION_APP: &str = "media_node";

/// `media_wanted`: the rendition files this node lacks and is to fetch from
/// a peer, with when to try next; this node's own, which a copy of the
/// store leaves alone (see [`super::fetcher`]).
pub const NODE_MIGRATIONS: Migrations<'static> = Migrations::new(
    NODE_MIGRATION_APP,
    &[Migration::up(
        "0001_media_wanted",
        "CREATE TABLE media_wanted (
            tenant_id TEXT NOT NULL,
            image_id TEXT NOT NULL,
            rendition TEXT NOT NULL,
            size INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            due_at INTEGER NOT NULL,
            attempts INTEGER NOT NULL,
            given_up INTEGER NOT NULL,
            reason TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, image_id, rendition)
        );
        CREATE INDEX media_wanted_due ON media_wanted (given_up, due_at);",
    )],
);
