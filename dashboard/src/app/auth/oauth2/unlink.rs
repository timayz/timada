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

use crate::app::auth::{account_url, error_code};
use crate::auth::{identity, require_user};

#[derive(Deserialize)]
struct UnlinkForm {
    provider: String,
    provider_user_id: String,
    /// The user the page was rendered for; must still be the signed-in user.
    user_id: String,
}

#[route(POST)]
pub async fn unlink(cx: &Cx, Form(form): Form<UnlinkForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    if form.user_id != user.id {
        return Ok(see_other(account_url(cx, Some("session_changed"), None)));
    }
    match identity(cx)
        .unlink_oauth2(&user.id, &form.provider, &form.provider_user_id)
        .await
    {
        Ok(()) => Ok(see_other(account_url(cx, None, Some("Provider unlinked.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(account_url(cx, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
