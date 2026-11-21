use std::sync::Arc;

use serde::Deserialize;
use timada_identity::{RootAccount, SignInRoot, normalize_email};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        request::client_ip,
        route,
    },
};

use crate::app::auth::{error_code, login_url, signed_in_url, verify_url};
use crate::auth::{
    cookies::{EmailFlow, EmailFlowKind, email_flow},
    current_user, identity, login_context, now, sign_in_as,
};
use crate::config::Config;
use crate::rate_limit::{RateLimiter, allow_password_login};

/// The refusal when an address or an account tried too many passwords.
pub const TOO_MANY_LOGINS: &str = "too_many_logins";

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
}

/// The first half of a password sign-in: the password is checked now, the
/// mailed code finishes it. Allowed while signed in (switching accounts).
/// Counted before the password is looked at, per browser address and per
/// account, so a password cannot be guessed by trying. Root's address is
/// the exception to the code: the configured password signs it in at once.
#[route(POST)]
pub async fn login(cx: &Cx, Form(form): Form<LoginForm>) -> Result<SeeOther> {
    let allowed = allow_password_login(
        context::<Arc<RateLimiter>>(cx),
        &context::<Config>(cx).auth,
        &form.email,
        client_ip(cx),
    )
    .await;
    if !allowed {
        tracing::info!("password sign-in refused: too many attempts");
        return Ok(see_other(login_url(cx, Some(TOO_MANY_LOGINS))));
    }
    let root = &context::<Config>(cx).auth.root;
    if root.has_email(&form.email) {
        return sign_in_root(cx, root, form).await;
    }
    let owner = match identity(cx)
        .check_password(&form.email, &form.password)
        .await
    {
        Ok(owner) => owner,
        Err(err) if err.is_user_facing() => {
            return Ok(see_other(login_url(cx, Some(error_code(&err)))));
        }
        Err(err) => return Err(err.into()),
    };
    // Valid: `check_password` normalized it already.
    let email = normalize_email(&form.email).map_err(topcoat::Error::from)?;

    match super::start_challenge(cx, &email, EmailFlowKind::SignIn).await? {
        Ok(started) => {
            email_flow(cx)
                .set(EmailFlow {
                    kind: EmailFlowKind::SignIn,
                    email,
                    password_hash: None,
                    user_id: Some(owner.user_id),
                    issued_at: now(),
                })
                .commit()?;
            Ok(see_other(verify_url(cx, None, started.message())))
        }
        Err(code) => Ok(see_other(login_url(cx, Some(code)))),
    }
}

/// Root, by the configuration's password and no mailed code.
async fn sign_in_root(cx: &Cx, root: &RootAccount, form: LoginForm) -> Result<SeeOther> {
    let signed_in = identity(cx)
        .sign_in_root(
            root,
            SignInRoot {
                email: form.email,
                password: form.password,
                session_user_id: current_user(cx).await.map(|user| user.id.clone()),
                context: login_context(cx),
            },
        )
        .await;
    match signed_in {
        Ok(done) => {
            sign_in_as(cx, &done.user_id).await?;
            Ok(see_other(signed_in_url(
                cx,
                &done.label,
                done.switched_from.is_some(),
            )))
        }
        Err(err) if err.is_user_facing() => Ok(see_other(login_url(cx, Some(error_code(&err))))),
        Err(err) => Err(err.into()),
    }
}
