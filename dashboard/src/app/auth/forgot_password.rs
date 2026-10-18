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
    view::{View, attributes, class, view},
};

use crate::app::auth::{forgot_password_url, verify_url};
use crate::auth::{
    cookies::{EmailFlow, EmailFlowKind, email_flow},
    now,
};
use crate::components::button::button;
use crate::components::card::CARD;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::ui::{LINK, auth_document, notice};

#[topcoat::router::query_params(error = bad_request)]
struct ForgotQuery {
    error: Option<String>,
}

#[page]
pub async fn forgot_password(cx: &Cx) -> Result<impl View> {
    let query = query_params::<ForgotQuery>(cx)?;
    Ok(view! {
        auth_document(
            title: "Forgot password",
            <div class=(class!(CARD, "gap-5 px-6"))>
                <div class="text-center">
                    <h1 class="text-2xl font-bold tracking-tight">"Reset your password"</h1>
                    <p class="mt-1 text-sm text-muted-foreground">"We will e-mail you a code to choose a new password."</p>
                </div>
                notice(error: query.error.as_deref())
                <form method="post" action=(href!(submit)) class="flex flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="email" }, "E-mail address")
                        input(attrs: attributes! {
                            id="email" name="email" type="email" autocomplete="username" maxlength="254" required=""
                        })
                    )
                    button(attrs: attributes! { class="w-full" }, "Send me a code")
                </form>
                <p class="text-center text-sm text-muted-foreground">
                    <a class=(LINK) href=(href!(super::login::login))>"Back to sign in"</a>
                </p>
            </div>
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
