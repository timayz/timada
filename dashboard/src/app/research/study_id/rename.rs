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
pub struct RenameForm {
    name: String,
}

/// Renames the study.
#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    match research(cx)
        .rename_study(&user_id, &study.id, &form.name)
        .await
    {
        Ok(()) => Ok(see_other(study_url(
            cx,
            &study.id,
            None,
            Some("Study renamed."),
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
