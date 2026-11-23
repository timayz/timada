//! Queries over `research_studies`, for the list of a user's studies.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

/// One row of `research_studies`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct StudyRow {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub country: String,
    pub language: String,
    pub seed_count: i64,
    /// How many keywords its seeds' last fetches gave together.
    pub keyword_count: i64,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

/// The statement of [`studies_of_user`].
pub(super) const STUDIES_OF_USER: &str =
    "SELECT id, owner_id, name, country, language, seed_count, keyword_count, created_at,
        updated_at
     FROM research_studies INDEXED BY research_studies_owner
     WHERE owner_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2";

/// The studies a user owns, the most recent first, `limit` at most.
pub async fn studies_of_user(db: &Db, user_id: &str, limit: i64) -> Result<Vec<StudyRow>> {
    db.fetch_all(STUDIES_OF_USER, (user_id, limit)).await
}
