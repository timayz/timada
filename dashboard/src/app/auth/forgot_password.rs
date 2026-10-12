//! `/auth/forgot-password`: a code mailed to a registered address lets its
//! owner choose a new password. The response never says whether the address
//! is registered.

use serde::Deserialize;
use timada_identity::normalize_email;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, view},
};

use crate::app::auth::{forgot_password_url, verify_url};
use crate::auth::{
    cookies::{EmailFlow, EmailFlowKind, email_flow},
    now,
};
use crate::ui::{document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct ForgotQuery {
    error: Option<String>,
}

#[page]
pub async fn forgot_password(cx: &Cx) -> Result<impl View> {
    let query = query_params::<ForgotQuery>(cx)?;
    Ok(view! {
        document(
            title: "Forgot password",
            <h1 class="mb-6 text-2xl font-semibold">"Reset your password"</h1>
            notice(error: query.error.as_deref())
            <form method="post" action=(href!(submit)) class="mb-6 space-y-3 rounded border border-neutral-200 bg-white p-4">
                <label class="block text-sm" for="email">"E-mail address"</label>
                <input id="email" name="email" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                    type="email" autocomplete="username" maxlength="254" required="">
                <p class="text-xs text-neutral-500">"We will e-mail you a code to choose a new password."</p>
                <button class="w-full rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Send me a code"</button>
            </form>
            <p class="text-sm">
                <a class="underline" href=(href!(super::login::login))>"Back to sign in"</a>
            </p>
        )
    })
}

#[derive(Deserialize)]
struct ForgotForm {
    email: String,
}

#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<ForgotForm>) -> Result<SeeOther> {
    let Ok(email) = normalize_email(&form.email) else {
        return Ok(see_other(forgot_password_url(cx, Some("invalid_email"))));
    };
    match super::email::start_challenge(cx, &email, EmailFlowKind::ResetPassword).await? {
        // Unknown address, or a code already on its way: same answer.
        Ok(_) | Err("not_registered") | Err("code_recently_sent") => {}
        Err(code) => return Ok(see_other(forgot_password_url(cx, Some(code)))),
    }
    email_flow(cx)
        .set(EmailFlow {
            kind: EmailFlowKind::ResetPassword,
            email,
            password_hash: None,
            user_id: None,
            issued_at: now(),
        })
        .commit()?;
    Ok(see_other(verify_url(
        cx,
        None,
        Some("If an account exists for this address, a code is on its way."),
    )))
}
