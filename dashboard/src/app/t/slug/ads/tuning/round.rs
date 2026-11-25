use timada_google::CAMPAIGN_SHOPPING;
use timada_tenancy::PERMISSION_ADS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::tuning_url;
use crate::app::t::slug::google::google;
use crate::app::t::slug::require_tenant;
use crate::campaigns::held;
use crate::tuning::may_tune;

/// Asks for a tuning round. The numbers are read and the proposals
/// prepared after, by `crate::tuning::subscription`; the page says what
/// came of it. Refused while the campaign is in its first weeks.
#[route(POST)]
pub async fn round(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_ADS_MANAGE).await?;
    let slug = access.tenant.slug.as_str();
    let campaign = held(access.executor()?, access.tenant_id(), CAMPAIGN_SHOPPING)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if let Err(err) = may_tune(campaign.as_ref(), crate::auth::now()) {
        return Ok(see_other(tuning_url(cx, slug, err.code(), None)));
    }
    match google(cx)
        .request_round(&access.user_id, access.tenant_id())
        .await
    {
        Ok(_) => Ok(see_other(tuning_url(
            cx,
            slug,
            None,
            Some("The round is being prepared."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(tuning_url(cx, slug, err.code(), None))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
