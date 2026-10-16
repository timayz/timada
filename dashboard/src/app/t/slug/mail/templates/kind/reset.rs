use timada_tenancy::PERMISSION_MAIL_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::{editor_url, require_kind};
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// The mail goes out in the built-in wording again.
#[route(POST)]
pub async fn reset(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let kind = require_kind(cx)?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .reset_mail_template(&access.user_id, &access.tenant.id, kind.key())
        .await
    {
        Ok(()) => Ok(see_other(editor_url(
            cx,
            slug,
            kind,
            None,
            Some("The mail goes out in the built-in wording again."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(editor_url(
            cx,
            slug,
            kind,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
