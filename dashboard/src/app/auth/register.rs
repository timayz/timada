//! `/auth/register`: an account from an e-mail address and a password. The
//! address is proven with a mailed code before anything is created.

use serde::Deserialize;
use timada_identity::{ChallengePurpose, hash_password, normalize_email};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, redirect, see_other},
        href, page, query_params, route,
    },
    view::{View, view},
};

use crate::app::auth::{error_code, home_url, register_url, verify_url};
use crate::auth::{
    cookies::{EmailFlow, email_flow},
    current_user, identity, now,
};
use crate::ui::{document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct RegisterQuery {
    error: Option<String>,
    /// The address typed before an error, to fill the form again.
    email: Option<String>,
}

#[page]
pub async fn register(cx: &Cx) -> Result<impl View> {
    if current_user(cx).await.is_some() {
        return Err(redirect(home_url(cx)).into());
    }
    let query = query_params::<RegisterQuery>(cx)?;

    Ok(view! {
        document(
            title: "Create an account",
            <h1 class="mb-6 text-2xl font-semibold">"Create your Timada account"</h1>
            notice(error: query.error.as_deref())

            <form method="post" action=(href!(submit)) class="mb-6 space-y-3 rounded border border-neutral-200 bg-white p-4">
                <label class="block text-sm" for="email">"E-mail address"</label>
                <input id="email" name="email" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                    type="email" autocomplete="email" maxlength="254" required="" value=(query.email.as_deref().unwrap_or_default())>
                <label class="block text-sm" for="password">"Password"</label>
                <input id="password" name="password" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                    type="password" autocomplete="new-password" minlength="8" maxlength="128" required="">
                <p class="text-xs text-neutral-500">"8 characters or more. We will e-mail you a code to confirm the address."</p>
                <button class="w-full rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Create account"</button>
            </form>

            <p class="text-sm">
                "Already have an account? "
                <a class="underline" href=(href!(super::login::login))>"Sign in"</a>
            </p>
        )
    })
}

#[derive(Deserialize)]
struct RegisterForm {
    email: String,
    password: String,
}

#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<RegisterForm>) -> Result<SeeOther> {
    if current_user(cx).await.is_some() {
        return Ok(see_other(home_url(cx)));
    }
    let Ok(email) = normalize_email(&form.email) else {
        return Ok(see_other(register_url(cx, Some("invalid_email"), None)));
    };
    // Say so now rather than after the person fetched a code for nothing.
    if identity(cx)
        .load_email_account(&email)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some()
    {
        return Ok(see_other(register_url(
            cx,
            Some("already_registered"),
            Some(&email),
        )));
    }
    let password_hash = match hash_password(&form.password).await {
        Ok(hash) => hash,
        Err(err) if err.is_user_facing() => {
            return Ok(see_other(register_url(
                cx,
                Some(error_code(&err)),
                Some(&email),
            )));
        }
        Err(err) => return Err(err.into()),
    };

    match super::email::start_challenge(cx, &email, ChallengePurpose::Register).await? {
        Ok(started) => {
            email_flow(cx)
                .set(EmailFlow {
                    purpose: ChallengePurpose::Register.as_str().to_owned(),
                    email,
                    password_hash: Some(password_hash),
                    user_id: None,
                    issued_at: now(),
                })
                .commit()?;
            Ok(see_other(verify_url(cx, None, started.message())))
        }
        Err(code) => Ok(see_other(register_url(cx, Some(code), Some(&email)))),
    }
}
