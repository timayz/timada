use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
    session,
};

use crate::app::auth::{account_url, error_code};
use crate::auth::{identity, require_user};

#[derive(Deserialize)]
struct RevokeForm {
    /// The user the page was rendered for; must still be the signed-in user.
    user_id: String,
}

/// Signs the user out of every other browser. This one keeps its session.
#[route(POST)]
pub async fn revoke(cx: &Cx, Form(form): Form<RevokeForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    if form.user_id != user.id {
        return Ok(see_other(account_url(cx, Some("session_changed"), None)));
    }
    let current = session::token_hash(cx).await?;
    match identity(cx)
        .revoke_sessions(&user.id, &user.id, current.as_deref())
        .await
    {
        Ok(()) => Ok(see_other(account_url(
            cx,
            None,
            Some("Signed out everywhere else."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(account_url(cx, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
