use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::feed_url;
use crate::app::t::slug::products::{catalog, error_code};
use crate::app::t::slug::require_tenant;

#[derive(serde::Deserialize)]
pub struct AdsLabelForm {
    /// The label's number, or empty for none.
    #[serde(default)]
    label: String,
}

/// Says which label of the feed marks the products of a category kept out
/// of ads.
#[route(POST)]
pub async fn ads_label(cx: &Cx, Form(form): Form<AdsLabelForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let label = match form.label.trim() {
        "" => None,
        label => match label.parse::<u8>() {
            Ok(label) => Some(label),
            Err(_) => {
                return Ok(see_other(feed_url(
                    cx,
                    slug,
                    Some("invalid_feed_label"),
                    None,
                )));
            }
        },
    };
    match catalog(&access)?
        .set_feed_ads_label(&access.user_id, label)
        .await
    {
        Ok(()) => Ok(see_other(feed_url(cx, slug, None, Some("Label saved.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(feed_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
