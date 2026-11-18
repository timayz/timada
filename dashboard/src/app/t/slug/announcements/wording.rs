use std::collections::HashMap;

use serde::Deserialize;
use timada_pages::PageError;
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

use super::{announcements_url, error_code, pages};
use crate::app::t::slug::require_tenant;

/// What the fields of a language's messages are named after: followed by
/// the message's id.
pub const TEXT_FIELD_PREFIX: &str = "text:";

#[derive(Deserialize)]
pub struct WordingForm {
    language: String,
    /// The messages, among anything else posted.
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

/// Words the bar's messages in a language: each one posted, an empty text
/// going back to the main language's.
#[route(POST)]
pub async fn wording(cx: &Cx, Form(form): Form<WordingForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let gateway = pages(&access)?;
    let worded = async {
        for (field, text) in &form.fields {
            if let Some(id) = field.strip_prefix(TEXT_FIELD_PREFIX) {
                gateway
                    .word_announcement(&access.user_id, &form.language, id, text)
                    .await?;
            }
        }
        Ok::<(), PageError>(())
    };
    match worded.await {
        Ok(()) => Ok(see_other(announcements_url(
            cx,
            slug,
            None,
            Some("Translations saved."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(announcements_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
