use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::members_url;
use crate::app::auth::home_url_with_message;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

#[derive(Deserialize)]
struct RevokeForm {
    user_id: String,
}

/// Removes a member, or lets the signed-in member leave (then the tenant's
/// pages are closed to them, so they land on the dashboard).
#[route(POST)]
pub async fn revoke(cx: &Cx, Form(form): Form<RevokeForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    let leaving = form.user_id == access.user_id;
    match tenancy(cx)
        .revoke_member(&access.user_id, &access.tenant.id, &form.user_id)
        .await
    {
        Ok(()) if leaving => Ok(see_other(home_url_with_message(
            cx,
            &format!("You left {}.", access.tenant.name),
        ))),
        Ok(()) => Ok(see_other(members_url(
            cx,
            slug,
            None,
            Some("Member removed."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(members_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
