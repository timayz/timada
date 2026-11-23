use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{require_owned, study_url};
use crate::app::research::{error_code, research};
use crate::keywords::Keywords;

#[derive(serde::Deserialize)]
pub struct FetchForm {
    /// The seed to fetch; without one, every seed that wants its keywords.
    #[serde(default)]
    seed: Option<String>,
}

/// Asks for keywords. The request is recorded here and answered off the
/// page (`crate::research`): the study's page shows the fetch on its way,
/// then its rows or why there are none.
#[route(POST)]
pub async fn request(cx: &Cx, Form(form): Form<FetchForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    if !context::<Keywords>(cx).enabled() {
        return Ok(see_other(study_url(
            cx,
            &study.id,
            Some("keywords_disabled"),
            None,
        )));
    }
    let seed = form.seed.as_deref().filter(|seed| !seed.trim().is_empty());
    match research(cx).request_fetch(&user_id, &study.id, seed).await {
        Ok(_) => Ok(see_other(study_url(
            cx,
            &study.id,
            None,
            Some("The keywords are being fetched."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(study_url(
            cx,
            &study.id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
