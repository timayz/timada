//! Schema of the read model, applied by the composition root in the
//! store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "writer";

/// `writer_drafts`: one row per draft a shop asked for. `to_check` holds
/// what the draft states that the shop's data does not, a line each. The
/// first index gives a shop's drafts in the order its page lists them, the
/// second the drafts of one product or category, latest first.
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_drafts",
        "CREATE TABLE writer_drafts (
            tenant_id TEXT NOT NULL,
            id TEXT NOT NULL,
            kind TEXT NOT NULL,
            subject_id TEXT NOT NULL,
            subject_name TEXT NOT NULL,
            keyword TEXT NOT NULL,
            requested_by TEXT NOT NULL,
            status TEXT NOT NULL,
            proposal_id TEXT NOT NULL,
            cost_usd_micros INTEGER NOT NULL,
            to_check TEXT NOT NULL,
            reason TEXT NOT NULL,
            requested_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (tenant_id, id)
        );
        CREATE INDEX writer_drafts_listed ON writer_drafts
            (tenant_id, requested_at DESC, id DESC);
        CREATE INDEX writer_drafts_subject ON writer_drafts
            (tenant_id, subject_id, requested_at DESC);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &["writer_drafts"];
