//! `/account/sign-in/code`: the shopper enters the code the shop mailed, and
//! is signed in on this browser.

use timada_storefront::{menu, request_sign_in_code, sign_in};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{RouterErrorExt, SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, view},
};

use crate::app::{document, say};
use crate::shopper::{forget_sign_in_flow, set_shopper_token, sign_in_flow};

#[topcoat::router::query_params(error = bad_request)]
struct CodeQuery {
    error: Option<String>,
    resent: Option<String>,
}

fn code_url(cx: &Cx, error: Option<&str>, resent: bool) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resent: Option<&'a str>,
    }
    href!(index)
        .query(Link {
            error,
            resent: resent.then_some("1"),
        })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let flow = sign_in_flow(cx).ok_or_redirect(super::sign_in_url(cx, None, None))?;
    let query = query_params::<CodeQuery>(cx)?;
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "Enter your code",
            nav: &nav,
            <h1>"Enter your code"</h1>
            <p>"We sent a six-digit code to " <strong>(&flow.email)</strong> ". It is valid for 10 minutes."</p>
            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            } else if query.resent.is_some() {
                <p class="notice" role="status">"A new code is on its way."</p>
            }
            <form class="sign-in" method="post" action=(href!(verify))>
                <label for="sign-in-code">"Code"</label>
                <input id="sign-in-code" name="code" required="" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9 ]*" maxlength="8">
                <button class="primary">"Sign in"</button>
            </form>
            <form method="post" action=(href!(verify))>
                <input type="hidden" name="resend" value="1">
                <button>"Send a new code"</button>
            </form>
            <p><a href=(super::sign_in_url(cx, Some(&flow.next), None))>"Use another address"</a></p>
        )
    })
}

#[derive(serde::Deserialize)]
pub struct CodeForm {
    #[serde(default)]
    code: String,
    #[serde(default)]
    resend: String,
}

/// Signs in with the code (or mails a new one), then on to where the
/// shopper was going.
#[route(POST)]
pub async fn verify(cx: &Cx, Form(form): Form<CodeForm>) -> Result<SeeOther> {
    let Some(flow) = sign_in_flow(cx) else {
        return Ok(see_other(super::sign_in_url(cx, None, None)));
    };
    if !form.resend.is_empty() {
        let error = request_sign_in_code(cx, &flow.email).await?.err();
        return Ok(see_other(code_url(
            cx,
            error.map(|refusal| refusal.code()),
            error.is_none(),
        )));
    }
    match sign_in(cx, &flow.email, &form.code).await? {
        Ok(signed_in) => {
            set_shopper_token(cx, &signed_in.token)?;
            forget_sign_in_flow(cx);
            Ok(see_other(flow.next))
        }
        Err(refusal) => Ok(see_other(code_url(cx, Some(refusal.code()), false))),
    }
}
