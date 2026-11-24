//! Queries over `research_keywords`, for a study's category tree: the
//! figures of the keywords it is made of.

use std::collections::HashMap;

use timada_core::db::{Db, FromRow, Value, params_from_iter};

use crate::score::Tier;
use crate::tree::KeywordFigures;

use super::keywords::DUPLICATE;

/// How many keywords one statement looks up.
const LOOKUPS_PER_STATEMENT: usize = 25;

/// The row a keyword is scored on.
#[derive(FromRow)]
struct Looked {
    keyword: String,
    volume: Option<i64>,
    cpc_usd_cents: Option<i64>,
    difficulty: Option<i64>,
    outcome: String,
}

/// The lookup of one keyword (`?{at}`) of a study (`?1`): the row it is
/// scored on, not the ones another seed gave too.
fn lookup(at: usize) -> String {
    format!(
        "SELECT keyword, volume, cpc_usd_cents, difficulty, outcome FROM research_keywords
         INDEXED BY research_keywords_keyword
         WHERE study_id = ?1 AND keyword = ?{at} AND outcome <> '{DUPLICATE}'"
    )
}

/// The statement of [`keyword_figures`] for `keywords` keywords: one seek
/// each, as a list of them would scan the study's rows.
pub(super) fn figures_statement(keywords: usize) -> String {
    (0..keywords)
        .map(|at| lookup(at + 2))
        .collect::<Vec<_>>()
        .join(" UNION ALL ")
}

/// The figures of `keywords` in a study, by keyword; one the read model
/// holds no row of is left out.
pub async fn keyword_figures(
    db: &Db,
    study_id: &str,
    keywords: &[String],
) -> anyhow::Result<HashMap<String, KeywordFigures>> {
    let mut figures = HashMap::with_capacity(keywords.len());
    for chunk in keywords.chunks(LOOKUPS_PER_STATEMENT) {
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(Value::from(study_id));
        values.extend(chunk.iter().map(|keyword| Value::from(keyword.as_str())));
        let rows: Vec<Looked> = db
            .fetch_all(&figures_statement(chunk.len()), params_from_iter(values))
            .await?;
        for row in rows {
            figures.insert(
                row.keyword,
                KeywordFigures {
                    volume: row.volume.map(|volume| volume as u64),
                    cpc_usd_cents: row.cpc_usd_cents.map(|cents| cents as u64),
                    difficulty: row
                        .difficulty
                        .map(|difficulty| difficulty.clamp(0, 100) as u8),
                    kept: row.outcome == Tier::Main.as_str()
                        || row.outcome == Tier::LongTail.as_str(),
                },
            );
        }
    }
    Ok(figures)
}
