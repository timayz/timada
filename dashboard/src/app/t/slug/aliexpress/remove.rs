use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::aliexpress_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Forgets what the shop buys on AliExpress: nothing is read from there any
/// more. The imported products stay as they are, and so does the company's
/// account.
#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .remove_aliexpress(&access.user_id, &access.tenant.id)
        .await
    {
        Ok(()) => Ok(see_other(aliexpress_url(
            cx,
            slug,
            None,
            Some("AliExpress imports stopped."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(aliexpress_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
