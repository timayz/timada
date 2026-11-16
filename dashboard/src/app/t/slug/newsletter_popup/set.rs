use serde::Deserialize;
use timada_pages::NewsletterPopupSettings;
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

/// The pop-up as the merchant fills it in; a box left unticked posts
/// nothing.
#[derive(Deserialize)]
pub struct PopupForm {
    #[serde(default)]
    shown: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    delay_secs: u32,
    #[serde(default)]
    on_leave: String,
}

/// Sets the pop-up as the form says and lands back on the page.
#[route(POST)]
pub async fn set(cx: &Cx, Form(form): Form<PopupForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let shown = !form.shown.is_empty();
    let settings = NewsletterPopupSettings {
        shown,
        title: form.title,
        text: form.text,
        delay_secs: form.delay_secs,
        on_leave: !form.on_leave.is_empty(),
    };
    let set = pages(&access)?
        .set_newsletter_popup(&access.user_id, settings)
        .await;
    match set {
        Ok(()) if shown => Ok(see_other(popup_url(
            cx,
            slug,
            None,
            Some("Pop-up saved: the storefront shows it in a moment."),
        ))),
        Ok(()) => Ok(see_other(popup_url(
            cx,
            slug,
            None,
            Some("Pop-up saved: the storefront does not show it."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(popup_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
