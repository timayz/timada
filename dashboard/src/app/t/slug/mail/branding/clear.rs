use timada_tenancy::PERMISSION_MAIL_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::super::mail_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// The shop's mails go back to the plain look.
#[route(POST)]
pub async fn clear(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .clear_mail_branding(&access.user_id, &access.tenant.id)
        .await
    {
        Ok(()) => Ok(see_other(mail_url(
            cx,
            slug,
            None,
            Some("Your mails are back to the plain look."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(mail_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
