use serde::Deserialize;
use timada_tenancy::PERMISSION_ADS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, not_found, see_other},
        route,
    },
};

use super::{campaigns_url, known};
use crate::app::t::slug::google::google;
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
pub struct LiveForm {
    #[serde(default)]
    kind: String,
}

/// Asks for a paused campaign to run. The member's click is the yes;
/// Google Ads is told after, by `crate::campaigns::subscription`, and the
/// page says how it went.
#[route(POST)]
pub async fn live(cx: &Cx, Form(form): Form<LiveForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_ADS_MANAGE).await?;
    let kind = known(&form.kind).ok_or_else(not_found)?;
    let slug = access.tenant.slug.as_str();
    match google(cx)
        .request_live(&access.user_id, access.tenant_id(), kind)
        .await
    {
        Ok(()) => Ok(see_other(campaigns_url(
            cx,
            slug,
            None,
            Some("Google Ads is being told to run the campaign."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(campaigns_url(cx, slug, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
