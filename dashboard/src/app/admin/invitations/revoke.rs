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

use crate::app::admin::users::{users_error_url, users_url};
use crate::app::admin::{error_code, require_admin};
use crate::auth::{identity, now};

#[derive(Deserialize)]
struct RevokeForm {
    invitation_id: String,
}

/// Withdraws an invitation still waiting; its link stops working.
#[route(POST)]
pub async fn revoke(cx: &Cx, Form(form): Form<RevokeForm>) -> Result<SeeOther> {
    let me = require_admin(cx).await?;
    match identity(cx)
        .revoke_account_invitation(&me.id, &form.invitation_id, now())
        .await
    {
        Ok(()) => Ok(see_other(users_url(cx, Some("Invitation withdrawn.")))),
        Err(err) if err.is_user_facing() => Ok(see_other(users_error_url(cx, error_code(&err)))),
        Err(err) => Err(err.into()),
    }
}
