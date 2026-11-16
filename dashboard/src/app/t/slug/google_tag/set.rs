use serde::Deserialize;
use timada_pages::GoogleTagSettings;
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

use super::google_tag_url;
use crate::app::t::slug::announcements::{error_code, pages};
use crate::app::t::slug::require_tenant;

/// The tag as the merchant fills it in; a box left unticked posts nothing.
#[derive(Deserialize)]
pub struct GoogleTagForm {
    #[serde(default)]
    analytics_id: String,
    #[serde(default)]
    ads_id: String,
    #[serde(default)]
    purchase_label: String,
    #[serde(default)]
    add_to_cart_label: String,
    #[serde(default)]
    begin_checkout_label: String,
    #[serde(default)]
    enhanced_conversions: String,
}

/// Sets the tag up as the form says and lands back on the page.
#[route(POST)]
pub async fn set(cx: &Cx, Form(form): Form<GoogleTagForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let settings = GoogleTagSettings {
        analytics_id: Some(form.analytics_id),
        ads_id: Some(form.ads_id),
        purchase_label: Some(form.purchase_label),
        add_to_cart_label: Some(form.add_to_cart_label),
        begin_checkout_label: Some(form.begin_checkout_label),
        enhanced_conversions: !form.enhanced_conversions.is_empty(),
    };
    let off = !settings
        .analytics_id
        .iter()
        .chain(&settings.ads_id)
        .any(|id| !id.trim().is_empty());
    let set = pages(&access)?
        .set_google_tag(&access.user_id, settings)
        .await;
    match set {
        Ok(()) if off => Ok(see_other(google_tag_url(
            cx,
            slug,
            None,
            Some("No tag: the storefront takes it off in a moment."),
        ))),
        Ok(()) => Ok(see_other(google_tag_url(
            cx,
            slug,
            None,
            Some("Tag saved: the storefront follows in a moment."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(google_tag_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
