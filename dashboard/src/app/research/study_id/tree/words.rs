use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::tree_url;
use crate::app::research::study_id::require_owned;
use crate::app::research::{error_code, research};

#[derive(serde::Deserialize)]
pub struct WordsForm {
    /// One a line; none checks no name.
    #[serde(default)]
    words: String,
}

/// Says the niche's own words, one of which every category's name is to
/// hold.
#[route(POST)]
pub async fn words(cx: &Cx, Form(form): Form<WordsForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    let back = |error: Option<&str>, message: Option<&str>| {
        Ok(see_other(format!(
            "{}#checks",
            tree_url(cx, &study.id, error, message)
        )))
    };
    let words: Vec<String> = form.words.lines().map(str::to_owned).collect();
    match research(cx)
        .set_niche_words(&user_id, &study.id, &words)
        .await
    {
        Ok(()) => back(None, Some("Saved: the names are checked again.")),
        Err(err) if err.is_user_facing() => back(Some(error_code(&err)), None),
        Err(err) => Err(err.into()),
    }
}
