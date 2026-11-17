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

use crate::app::words::t;
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
    let title = t!(account_code_title).resolve(cx);
    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(t!(account_code_title))</h1>
                <p>(t!(account_code_sent_before)) " " <strong>(&flow.email)</strong> (t!(account_code_sent_after))</p>
                if let Some(code) = query.error.as_deref() {
                    <p class="notice" role="alert">(say(code))</p>
                } else if query.resent.is_some() {
                    <p class="notice" role="status">(t!(account_code_resent))</p>
                }
                <form class="sign-in" method="post" action=(href!(verify))>
                    <label for="sign-in-code">(t!(account_code_label))</label>
                    <input id="sign-in-code" name="code" required="" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9 ]*" maxlength="8">
                    <button class="primary">(t!(account_code_submit))</button>
                </form>
                <form method="post" action=(href!(verify))>
                    <input type="hidden" name="resend" value="1">
                    <button>(t!(account_code_resend))</button>
                </form>
                <p><a href=(super::sign_in_url(cx, Some(&flow.next), None))>(t!(account_code_other_address))</a></p>
            </div>
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
