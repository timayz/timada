use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{PARENT, PARK, REMOVE, UNDER, tree_url};
use crate::app::research::study_id::require_owned;
use crate::app::research::{error_code, research};
use crate::db::Db;

/// A keyword's row says where it goes.
#[derive(serde::Deserialize)]
pub struct PlaceForm {
    keyword: String,
    /// [`PARENT`], [`UNDER`] followed by a parent category's keyword,
    /// [`PARK`] or [`REMOVE`].
    to: String,
    /// The list of kept keywords the page was on, to come back to.
    #[serde(default)]
    list: Option<String>,
}

/// Puts a keyword in the study's category tree, parks it, or takes it out.
/// Only a keyword the score keeps goes in: the read model says which.
#[route(POST)]
pub async fn place(cx: &Cx, Form(form): Form<PlaceForm>) -> Result<SeeOther> {
    let (study, user_id) = require_owned(cx).await?;
    let list = form.list.as_deref();
    let back = |error: Option<&str>, message: Option<&str>| {
        Ok(see_other(tree_url(cx, &study.id, list, error, message)))
    };
    let research = research(cx);
    let keyword = form.keyword.as_str();
    let parent = match form.to.as_str() {
        PARENT => None,
        to => to.strip_prefix(UNDER),
    };
    let entering = form.to == PARENT || parent.is_some();
    if !entering && form.to != PARK && form.to != REMOVE {
        return back(Some("unknown_parent_category"), None);
    }
    if entering && study.tree.category(keyword).is_none() {
        let figures =
            timada_research::keyword_figures(context::<Db>(cx), &study.id, &[keyword.to_owned()])
                .await
                .map_err(topcoat::Error::from_anyhow)?;
        if !figures.get(keyword).is_some_and(|figures| figures.kept) {
            return back(Some("keyword_not_kept"), None);
        }
    }
    let (done, message) = if entering {
        (
            research
                .place_category(&user_id, &study.id, keyword, parent)
                .await,
            "Placed in the tree.",
        )
    } else if form.to == PARK {
        (
            research.park_category(&user_id, &study.id, keyword).await,
            "Parked for later.",
        )
    } else {
        (
            research.remove_category(&user_id, &study.id, keyword).await,
            "Taken out.",
        )
    };
    match done {
        Ok(()) => back(None, Some(message)),
        Err(err) if err.is_user_facing() => back(Some(error_code(&err)), None),
        Err(err) => Err(err.into()),
    }
}
