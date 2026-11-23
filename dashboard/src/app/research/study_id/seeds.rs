use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{require_owned, study_url};
use crate::app::research::{error_code, research};

#[derive(serde::Deserialize)]
pub struct SeedForm {
    seed: String,
}

/// Gives the study one more seed. Its keywords are fetched when asked for.
#[route(POST)]
pub async fn add_seed(cx: &Cx, Form(form): Form<SeedForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    match research(cx).add_seed(&user_id, &study.id, &form.seed).await {
        Ok(()) => Ok(see_other(study_url(
            cx,
            &study.id,
            None,
            Some("Seed added."),
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
