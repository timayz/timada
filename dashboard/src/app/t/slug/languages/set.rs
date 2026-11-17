//! `POST /t/{slug}/languages/set`: the storefront's languages, the main one
//! and any others, each a checkbox named `language:<code>`.

use std::collections::HashMap;

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

use super::languages_url;
use crate::app::t::slug::announcements::{error_code, pages};
use crate::app::t::slug::require_tenant;

/// What the checkboxes of the other languages are named after.
pub const LANGUAGE_FIELD_PREFIX: &str = "language:";

#[derive(Deserialize)]
pub struct LanguagesForm {
    #[serde(default)]
    main: String,
    /// The ticked language checkboxes, among anything else posted.
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

/// Saves the languages; a code not in the list is refused.
#[route(POST)]
pub async fn set(cx: &Cx, Form(form): Form<LanguagesForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let others: Vec<String> = form
        .fields
        .keys()
        .filter_map(|key| key.strip_prefix(LANGUAGE_FIELD_PREFIX))
        .map(str::to_owned)
        .collect();
    match pages(&access)?
        .set_languages(&access.user_id, &form.main, &others)
        .await
    {
        Ok(()) => Ok(see_other(languages_url(
            cx,
            slug,
            None,
            Some("Languages saved: the storefront follows in a moment."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(languages_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
