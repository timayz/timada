//! `/account/sign-in`: the shopper gives their e-mail address and the shop
//! mails them a code (`/account/sign-in/code` takes it). There is no
//! sign-up: the first code entered creates the account.

pub mod code;

use timada_storefront::{menu, request_sign_in_code};
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

use crate::app::words::t;
use crate::app::{document, say};
use crate::shopper::{SignInFlow, safe_next, set_sign_in_flow, sign_in_flow};

#[topcoat::router::query_params(error = bad_request)]
struct SignInQuery {
    next: Option<String>,
    error: Option<String>,
}

/// The sign-in page, coming back to `next` once signed in.
pub fn sign_in_url(cx: &Cx, next: Option<&str>, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        next: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
    }
    href!(index).query(Link { next, error }).resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<SignInQuery>(cx)?;
    let nav = menu(cx).await?;
    let next = safe_next(query.next.as_deref(), href!(super::index).resolve(cx));
    let email = sign_in_flow(cx).map(|flow| flow.email).unwrap_or_default();
    let title = t!(shell_sign_in).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(t!(shell_sign_in))</h1>
                <p>(t!(account_sign_in_text))</p>
                if let Some(code) = query.error.as_deref() {
                    <p class="notice" role="alert">(say(code))</p>
                }
                <form class="sign-in" method="post" action=(href!(request))>
                    <input type="hidden" name="next" value=(&next)>
                    <label for="sign-in-email">(t!(account_sign_in_email))</label>
                    <input id="sign-in-email" name="email" type="email" required="" autocomplete="email" value=(&email)>
                    <button class="primary">(t!(account_sign_in_request))</button>
                </form>
            </div>
        )
    })
}

#[derive(serde::Deserialize)]
pub struct RequestForm {
    email: String,
    #[serde(default)]
    next: String,
}

/// Mails the code, then asks for it. A code mailed less than a minute ago
/// is still the one to enter.
#[route(POST)]
pub async fn request(cx: &Cx, Form(form): Form<RequestForm>) -> Result<SeeOther> {
    let next = safe_next(Some(&form.next), href!(super::index).resolve(cx));
    match request_sign_in_code(cx, &form.email).await? {
        Ok(()) => {}
        Err(refusal) if refusal.code() == "code_recently_sent" => {}
        Err(refusal) => {
            return Ok(see_other(sign_in_url(
                cx,
                Some(&next),
                Some(refusal.code()),
            )));
        }
    }
    set_sign_in_flow(
        cx,
        SignInFlow {
            email: form.email.trim().to_owned(),
            next,
        },
    )?;
    Ok(see_other(href!(code::index).resolve(cx)))
}
