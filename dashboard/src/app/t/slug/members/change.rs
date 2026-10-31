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
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

#[derive(Deserialize)]
struct ChangeForm {
    user_id: String,
    role: String,
}

/// Moves a member to another role; the domain lets owners alone do it and
/// keeps the last owner.
#[route(POST)]
pub async fn change(cx: &Cx, Form(form): Form<ChangeForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .change_member_role(
            &access.user_id,
            &access.tenant.id,
            &form.user_id,
            &form.role,
        )
        .await
    {
        Ok(()) => Ok(see_other(members_url(
            cx,
            slug,
            None,
            Some("Role changed."),
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
