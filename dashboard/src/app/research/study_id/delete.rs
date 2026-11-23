use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::{require_owned, study_url};
use crate::app::research::{error_code, research, research_url};

/// Deletes the study; its owner goes back to the list.
#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    match research(cx).delete_study(&user_id, &study.id).await {
        Ok(()) => Ok(see_other(research_url(cx, None, Some("Study deleted.")))),
        Err(err) if err.is_user_facing() => Ok(see_other(study_url(
            cx,
            &study.id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
