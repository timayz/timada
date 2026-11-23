use timada_research::main_keywords;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{competition_url, require_owned};
use crate::app::research::{error_code, research};
use crate::db::Db;
use crate::keywords::Keywords;

#[derive(serde::Deserialize)]
pub struct CheckForm {
    /// `yes` searches every main keyword again; else only those that still
    /// want their first results.
    #[serde(default)]
    again: Option<String>,
}

/// Asks who holds the first results of the study's main keywords, as the
/// score names them now. The request is recorded here and answered off the
/// page (`crate::research`), one paid search per keyword.
#[route(POST)]
pub async fn check(cx: &Cx, Form(form): Form<CheckForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    if !context::<Keywords>(cx).enabled() {
        return Ok(see_other(competition_url(
            cx,
            &study.id,
            Some("keywords_disabled"),
            None,
        )));
    }
    let mains = main_keywords(context::<Db>(cx), &study.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let again = form.again.as_deref() == Some("yes");
    match research(cx)
        .request_results(&user_id, &study.id, &mains, again)
        .await
    {
        Ok(_) => Ok(see_other(competition_url(
            cx,
            &study.id,
            None,
            Some("The first results are being searched."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(competition_url(
            cx,
            &study.id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
