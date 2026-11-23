use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{competition_url, require_owned};
use crate::app::research::{error_code, research};

#[derive(serde::Deserialize)]
pub struct MarkForm {
    domain: String,
    /// `yes` for a specialist shop, anything else for a generalist.
    specialist: String,
}

/// Marks a site as a specialist shop or as a generalist. The mark is the
/// user's: it counts on this study and on every other of theirs.
#[route(POST)]
pub async fn mark(cx: &Cx, Form(form): Form<MarkForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    match research(cx)
        .mark_domain(&user_id, &form.domain, form.specialist == "yes")
        .await
    {
        Ok(()) => Ok(see_other(competition_url(cx, &study.id, None, None))),
        Err(err) if err.is_user_facing() => Ok(see_other(competition_url(
            cx,
            &study.id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
