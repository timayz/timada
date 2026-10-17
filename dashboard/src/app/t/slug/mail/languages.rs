//! `POST /t/{slug}/mail/languages`: the languages the shop mails in, a
//! default and any others, each a checkbox named `language:<code>`.

use std::collections::HashMap;

use serde::Deserialize;
use timada_tenancy::{PERMISSION_MAIL_MANAGE, SetMailLanguages};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::mail_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// What the checkboxes of the other languages are named after.
pub const LANGUAGE_FIELD_PREFIX: &str = "language:";

#[derive(Deserialize)]
pub struct LanguagesForm {
    default: String,
    /// The ticked language checkboxes, among anything else posted.
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

/// Saves the languages; a code not in the list is refused.
#[route(POST)]
pub async fn save(cx: &Cx, Form(form): Form<LanguagesForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let others = form
        .fields
        .keys()
        .filter_map(|key| key.strip_prefix(LANGUAGE_FIELD_PREFIX))
        .map(str::to_owned)
        .collect();
    match tenancy(cx)
        .set_mail_languages(
            &access.user_id,
            &access.tenant.id,
            SetMailLanguages {
                default: form.default,
                others,
            },
        )
        .await
    {
        Ok(()) => Ok(see_other(mail_url(
            cx,
            slug,
            None,
            Some("Saved: your mails go out in these languages."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(mail_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
