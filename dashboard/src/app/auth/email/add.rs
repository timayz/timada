//! `POST /auth/email/add`: a signed-in user who registered with a provider or
//! a passkey chooses an address and a password. The address is proven with a
//! mailed code before the method is added; it cannot be changed afterwards.

use serde::Deserialize;
use timada_identity::{hash_password, normalize_email};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::auth::{account_url, error_code, verify_url};
use crate::auth::{
    cookies::{EmailFlow, EmailFlowKind, email_flow},
    identity, now, require_user,
};

#[derive(Deserialize)]
struct AddForm {
    /// The user the page was rendered for; must still be the signed-in user.
    user_id: String,
    email: String,
    password: String,
}

#[route(POST)]
pub async fn add(cx: &Cx, Form(form): Form<AddForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    if form.user_id != user.id {
        return Ok(see_other(account_url(cx, Some("session_changed"), None)));
    }
    if user.password.is_some() {
        return Ok(see_other(account_url(cx, Some("email_set"), None)));
    }
    let Ok(email) = normalize_email(&form.email) else {
        return Ok(see_other(account_url(cx, Some("invalid_email"), None)));
    };
    // Say so now rather than after the person fetched a code for nothing.
    if identity(cx)
        .load_email_account(&email)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some()
    {
        return Ok(see_other(account_url(cx, Some("already_registered"), None)));
    }
    let password_hash = match hash_password(&form.password).await {
        Ok(hash) => hash,
        Err(err) if err.is_user_facing() => {
            return Ok(see_other(account_url(cx, Some(error_code(&err)), None)));
        }
        Err(err) => return Err(err.into()),
    };

    match super::start_challenge(cx, &email, EmailFlowKind::AddEmail).await? {
        Ok(started) => {
            email_flow(cx)
                .set(EmailFlow {
                    kind: EmailFlowKind::AddEmail,
                    email,
                    password_hash: Some(password_hash),
                    user_id: Some(user.id.clone()),
                    issued_at: now(),
                })
                .commit()?;
            Ok(see_other(verify_url(cx, None, started.message())))
        }
        Err(code) => Ok(see_other(account_url(cx, Some(code), None))),
    }
}
