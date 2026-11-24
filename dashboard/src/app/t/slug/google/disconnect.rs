use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::{error_code, google, google_url};
use crate::app::t::slug::require_tenant;

/// Forgets the shop's grant: nothing is read from Google for it any more.
/// The account's yes stays at Google until the account withdraws it there
/// (see `crate::google`).
#[route(POST)]
pub async fn disconnect(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match google(cx)
        .disconnect(&access.user_id, access.tenant_id())
        .await
    {
        Ok(()) => Ok(see_other(google_url(
            cx,
            slug,
            None,
            Some("Google account disconnected."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(google_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
