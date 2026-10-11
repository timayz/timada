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
struct RemoveForm {
    credential_id: String,
    user_id: String,
}

#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<RemoveForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    if form.user_id != user.id {
        return Ok(see_other(account_url(cx, Some("session_changed"), None)));
    }
    match identity(cx)
        .remove_passkey(&user.id, &form.credential_id)
        .await
    {
        Ok(()) => Ok(see_other(account_url(cx, None, Some("Passkey removed.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(account_url(cx, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
