use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::admin::users::{users_error_url, users_url};
use crate::app::admin::{error_code, require_admin};
use crate::auth::{identity, now};
use crate::seal::InvitationTokens;

#[derive(Deserialize)]
struct InviteForm {
    email: String,
}

/// Records an invitation to create an account; its link is mailed by the
/// subscription that follows the stream
/// ([`crate::account_invitation_mails`]), and the page of users says how
/// that went. Root alone invites: the domain refuses anyone else.
#[route(POST)]
pub async fn send(cx: &Cx, Form(form): Form<InviteForm>) -> Result<SeeOther> {
    let me = require_admin(cx).await?;
    match identity(cx)
        .invite_account(
            &*context::<InvitationTokens>(cx).0,
            &me.id,
            &form.email,
            now(),
        )
        .await
    {
        Ok(sent) => Ok(see_other(users_url(
            cx,
            Some(&format!(
                "Sending the invitation to {}…",
                sent.invitation.email
            )),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(users_error_url(cx, error_code(&err)))),
        Err(err) => Err(err.into()),
    }
}
