use serde::Deserialize;
use timada_identity::{ChallengePurpose, normalize_email};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::auth::{error_code, login_url, verify_url};
use crate::auth::{
    cookies::{EmailFlow, email_flow},
    identity, now,
};

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
}

/// The first half of a password sign-in: the password is checked now, the
/// mailed code finishes it. Allowed while signed in (switching accounts).
#[route(POST)]
pub async fn login(cx: &Cx, Form(form): Form<LoginForm>) -> Result<SeeOther> {
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

    match super::start_challenge(cx, &email, ChallengePurpose::SignIn).await? {
        Ok(started) => {
            email_flow(cx)
                .set(EmailFlow {
                    purpose: ChallengePurpose::SignIn.as_str().to_owned(),
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
