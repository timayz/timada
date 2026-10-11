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
struct RenameForm {
    credential_id: String,
    user_id: String,
    name: String,
}

#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    if form.user_id != user.id {
        return Ok(see_other(account_url(cx, Some("session_changed"), None)));
    }
    match identity(cx)
        .rename_passkey(&user.id, &form.credential_id, &form.name)
        .await
    {
        Ok(()) => Ok(see_other(account_url(cx, None, Some("Passkey renamed.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(account_url(cx, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
