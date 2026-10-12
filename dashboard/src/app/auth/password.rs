//! `POST /auth/password`: a new password from the account page, proven with
//! the current one. Like a reset, it signs the user's other browsers out.

use serde::Deserialize;
use timada_identity::{ChangePassword, hash_password};
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
struct PasswordForm {
    /// The user the page was rendered for; must still be the signed-in user.
    user_id: String,
    current_password: String,
    password: String,
    password_confirm: String,
}

#[route(POST)]
pub async fn change(cx: &Cx, Form(form): Form<PasswordForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    if form.user_id != user.id {
        return Ok(see_other(account_url(cx, Some("session_changed"), None)));
    }
    if form.password_confirm != form.password {
        return Ok(see_other(account_url(cx, Some("password_mismatch"), None)));
    }
    let new_password_hash = match hash_password(&form.password).await {
        Ok(hash) => hash,
        Err(err) if err.is_user_facing() => {
            return Ok(see_other(account_url(cx, Some(error_code(&err)), None)));
        }
        Err(err) => return Err(err.into()),
    };
    let changed = identity(cx)
        .change_password(ChangePassword {
            user_id: user.id.clone(),
            current_password: form.current_password,
            new_password_hash,
        })
        .await;
    match changed {
        Ok(()) => {
            let current = session::token_hash(cx).await?;
            identity(cx)
                .revoke_sessions(&user.id, &user.id, current.as_deref())
                .await?;
            Ok(see_other(account_url(
                cx,
                None,
                Some("Password changed. Other browsers were signed out."),
            )))
        }
        Err(err) if err.is_user_facing() => {
            Ok(see_other(account_url(cx, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
