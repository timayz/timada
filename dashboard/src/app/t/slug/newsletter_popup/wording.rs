use serde::Deserialize;
use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::popup_url;
use crate::app::t::slug::announcements::{error_code, pages};
use crate::app::t::slug::require_tenant;

/// What the pop-up says in one of the shop's other languages.
#[derive(Deserialize)]
pub struct PopupWordingForm {
    language: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    text: String,
}

/// Words the pop-up in a language; what is left empty is read in the main
/// one.
#[route(POST)]
pub async fn wording(cx: &Cx, Form(form): Form<PopupWordingForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let worded = pages(&access)?
        .word_newsletter_popup(&access.user_id, &form.language, &form.title, &form.text)
        .await;
    match worded {
        Ok(()) => Ok(see_other(popup_url(
            cx,
            slug,
            None,
            Some("Translation saved."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(popup_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
