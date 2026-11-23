use timada_research::{Thresholds, judge, study_row};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{response::IntoResponse, route},
};

use super::require_owned;
use crate::db::Db;
use crate::export::download_headers;
use crate::research::study_csv;

/// The study's keywords with what the score made of each, as a CSV file,
/// for its owner: the report of one study, not the keyword source's data
/// passed on (`docs/keywords.md`).
#[route(GET)]
pub async fn export(cx: &Cx) -> Result<impl IntoResponse> {
    let (study, _) = require_owned(cx).await?;
    let db = context::<Db>(cx).clone();
    let tally = study_row(&db, &study.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .map(|row| row.tally())
        .unwrap_or_default();
    let judgement = judge(&tally, &study.inputs, &Thresholds::default());
    let name = format!("study-{}.csv", study.id.to_lowercase());
    Ok((download_headers(&name), study_csv(db, study, judgement)))
}
