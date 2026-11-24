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
    /// `go`, `no_go` or `incomplete`, as the study was last scored.
    pub verdict: String,
    /// How many distinct keywords were scored.
    pub scored_keywords: i64,
    pub main_count: i64,
    pub long_tail_count: i64,
    /// Searches a month over the kept keywords.
    pub kept_volume: i64,
    /// The kept keywords' cost per click averaged by volume, in thousandths
    /// of a cent of `currency`.
    pub avg_cpc_millis: Option<i64>,
    /// The currency of the sale; empty until the owner said one.
    pub currency: String,
    /// The return on ad spend at which an ad pays for itself, in
    /// hundredths; `None` without a sale, or when it leaves nothing.
    pub break_even_roas: Option<i64>,
    /// The sale's price over the average cost per click, in hundredths.
    pub price_per_click: Option<i64>,
    /// What that ratio must reach for an ad to pay, in hundredths.
    pub needed_price_per_click: Option<i64>,
    /// Where the competition test stood when it was last worked out:
    /// `passed`, `failed` or `not_checked`.
    pub competition: String,
    /// How many of its main keywords have their first results.
    pub searched_count: i64,
    /// How many are still without.
    pub owed_count: i64,
    /// The shop its categories are created in; empty until it has one.
    pub tenant_id: String,
    /// How many categories it created there.
    pub created_count: i64,
}

impl StudyRow {
    /// What its keywords came to when it was last scored.
    pub fn tally(&self) -> crate::Tally {
        crate::Tally {
            keywords: self.scored_keywords as u64,
            main: self.main_count as u32,
            long_tail: self.long_tail_count as u32,
            kept_volume: self.kept_volume as u64,
            avg_cpc_millis: self.avg_cpc_millis.map(|millis| millis as u64),
        }
    }
}

const STUDY_COLUMNS: &str =
    "SELECT id, owner_id, name, country, language, seed_count, keyword_count, created_at,
        updated_at, verdict, scored_keywords, main_count, long_tail_count, kept_volume,
        avg_cpc_millis, currency, break_even_roas, price_per_click, needed_price_per_click,
        competition, searched_count, owed_count, tenant_id, created_count
     FROM research_studies";

/// The statement of [`studies_of_user`].
pub(super) fn studies_of_user_statement() -> String {
    format!(
        "{STUDY_COLUMNS} INDEXED BY research_studies_owner
         WHERE owner_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2"
    )
}

/// The studies a user owns, the most recent first, `limit` at most.
pub async fn studies_of_user(db: &Db, user_id: &str, limit: i64) -> Result<Vec<StudyRow>> {
    db.fetch_all(&studies_of_user_statement(), (user_id, limit))
        .await
}

/// The row of one study, as the read model last wrote it; `None` before
/// its first event got there, and once it is deleted.
pub async fn study_row(db: &Db, study_id: &str) -> Result<Option<StudyRow>> {
    Ok(db
        .fetch_all(&format!("{STUDY_COLUMNS} WHERE id = ?1"), (study_id,))
        .await?
        .into_iter()
        .next())
}

/// The statement of [`study_of_shop`].
pub(super) fn study_of_shop_statement() -> String {
    format!(
        "{STUDY_COLUMNS} INDEXED BY research_studies_shop
         WHERE tenant_id = ?1 ORDER BY created_at DESC LIMIT 1"
    )
}

/// The study a shop's categories were created from: the niche it starts
/// with. The most recent, should two name the shop; `None` for a shop made
/// by hand.
pub async fn study_of_shop(db: &Db, tenant_id: &str) -> Result<Option<StudyRow>> {
    if tenant_id.is_empty() {
        return Ok(None);
    }
    Ok(db
        .fetch_all(&study_of_shop_statement(), (tenant_id,))
        .await?
        .into_iter()
        .next())
}
