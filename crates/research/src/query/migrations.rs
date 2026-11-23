//! Schema of the read model: one migration, applied by the composition root
//! in the store's database next to evento's own.

use evento::turso::migrate::{Migration, Migrations};

/// The app name under which these migrations are recorded.
pub const MIGRATION_APP: &str = "research";

/// `research_studies`: one row per study, listed by owner.
/// `research_keywords`: one row per keyword a seed's last fetch gave, read
/// by study from the most searched down (`sort_volume` is the volume, 0
/// when the source has none; `id` orders the rows of one volume).
pub const MIGRATIONS: Migrations<'static> = Migrations::new(
    MIGRATION_APP,
    &[Migration::up(
        "0001_studies",
        "CREATE TABLE research_studies (
            id TEXT NOT NULL PRIMARY KEY,
            owner_id TEXT NOT NULL,
            name TEXT NOT NULL,
            country TEXT NOT NULL,
            language TEXT NOT NULL,
            seed_count INTEGER NOT NULL,
            keyword_count INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX research_studies_owner ON research_studies
            (owner_id, created_at DESC, id DESC);
        CREATE TABLE research_keywords (
            study_id TEXT NOT NULL,
            id TEXT NOT NULL,
            seed TEXT NOT NULL,
            keyword TEXT NOT NULL,
            volume INTEGER,
            cpc_usd_cents INTEGER,
            difficulty INTEGER,
            intent TEXT,
            sort_volume INTEGER NOT NULL,
            fetched_at INTEGER NOT NULL,
            PRIMARY KEY (study_id, id)
        );
        CREATE INDEX research_keywords_volume ON research_keywords
            (study_id, sort_volume DESC, id DESC);
        CREATE INDEX research_keywords_seed ON research_keywords (study_id, seed);",
    )],
);

/// The tables the migration creates: what a node empties before copying its
/// store from a peer, so the subscription fills them again.
pub const TABLES: &[&str] = &["research_studies", "research_keywords"];
