//! `/auth/register`: an account from an e-mail address and a password. The
//! address is proven with a mailed code before anything is created.

use serde::Deserialize;
use timada_identity::{hash_password, normalize_email};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, redirect, see_other},
        href, page, query_params, route,
    },
    view::{View, attributes, class, view},
};

use crate::app::auth::{error_code, home_url, register_url, verify_url};
use crate::auth::{
    cookies::{EmailFlow, EmailFlowKind, email_flow},
    current_user, identity, now,
};
use crate::components::button::button;
use crate::components::card::CARD;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::ui::{LINK, auth_document, notice};

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
        auth_document(
            title: "Create an account",
            <div class=(class!(CARD, "gap-5 px-6"))>
                <div class="text-center">
                    <h1 class="text-2xl font-bold tracking-tight">"Create your Timada account"</h1>
                    <p class="mt-1 text-sm text-muted-foreground">"We will e-mail you a code to confirm the address."</p>
                </div>
                notice(error: query.error.as_deref())

                <form method="post" action=(href!(submit)) class="flex flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="email" }, "E-mail address")
                        input(attrs: attributes! {
                            id="email" name="email" type="email" autocomplete="email" maxlength="254" required=""
                            value=(query.email.as_deref().unwrap_or_default())
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="password" }, "Password")
                        input(attrs: attributes! {
                            id="password" name="password" type="password" autocomplete="new-password" minlength="8" maxlength="128" required=""
                        })
                        field_description("8 characters or more.")
                    )
                    button(attrs: attributes! { class="w-full" }, "Create account")
                </form>

                <p class="text-center text-sm text-muted-foreground">
                    "Already have an account? "
                    <a class=(LINK) href=(href!(super::login::login))>"Sign in"</a>
                </p>
            </div>
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

    match super::email::start_challenge(cx, &email, EmailFlowKind::Register).await? {
        Ok(started) => {
            email_flow(cx)
                .set(EmailFlow {
                    kind: EmailFlowKind::Register,
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
