//! `POST /t/{slug}/redirects/{redirect_id}/remove`: stops the redirect.

use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::redirect_id;
use crate::app::t::slug::announcements::{error_code, pages};
use crate::app::t::slug::redirects::redirects_url;
use crate::app::t::slug::require_tenant;

#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let removed = pages(&access)?
        .remove_redirect(&access.user_id, redirect_id(cx))
        .await;
    match removed {
        Ok(()) => Ok(see_other(redirects_url(
            cx,
            slug,
            None,
            Some("Redirect deleted."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(redirects_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
