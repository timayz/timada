use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{KeywordForm, list_url, require_owned};
use crate::app::research::{error_code, research};

/// Brings a keyword back, whatever dropped it. The study is scored again, off the page.
#[route(POST)]
pub async fn restore(cx: &Cx, Form(form): Form<KeywordForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    let list = form.list.as_deref();
    match research(cx)
        .restore_keyword(&user_id, &study.id, &form.keyword)
        .await
    {
        Ok(()) => Ok(see_other(list_url(
            cx,
            &study.id,
            list,
            None,
            Some("Keyword brought back."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(list_url(
            cx,
            &study.id,
            list,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
