use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::feed_url;
use crate::app::t::slug::products::{catalog, error_code};
use crate::app::t::slug::require_tenant;

/// Opens the feed, or moves an open one to a new address.
#[route(POST)]
pub async fn open(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match catalog(&access)?
        .open_feed(&access.user_id)
        .await
        .map(|_| ())
    {
        Ok(()) => Ok(see_other(feed_url(
            cx,
            slug,
            None,
            Some("The feed is open at a new address."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(feed_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
