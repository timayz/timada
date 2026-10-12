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
use crate::auth::now;

#[derive(Deserialize)]
struct RevokeInvitationForm {
    invitation_id: String,
}

/// Withdraws a pending invitation; its link stops working.
#[route(POST)]
pub async fn revoke_invitation(
    cx: &Cx,
    Form(form): Form<RevokeInvitationForm>,
) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .revoke_invitation(&access.user_id, &form.invitation_id, now())
        .await
    {
        Ok(()) => Ok(see_other(members_url(
            cx,
            slug,
            None,
            Some("Invitation withdrawn."),
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
