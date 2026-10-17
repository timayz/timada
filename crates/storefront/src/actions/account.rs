//! The shopper's account: asking for a sign-in code, entering it, signing
//! out (here, or everywhere else), deleting the account, forgetting a saved
//! address.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::content::Form;
use topcoat_router::error::{SeeOther, see_other};

use crate::messages::SIGNED_OUT_ELSEWHERE;
use crate::session::{
    SignInFlow, forget_checkout_draft, forget_shopper, forget_sign_in_flow, set_shopper_token,
    set_sign_in_flow, shopper_token, sign_in_flow,
};
use crate::shoppers::{
    delete_account, remove_address, request_sign_in_code, sign_in, sign_out as end_session,
    sign_out_everywhere_else,
};
use crate::storefront::paths;
use crate::urls::safe_next;

use super::action;

/// What the sign-in form posts: the address, and where to go after.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RequestForm {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub next: String,
}

/// Mails the code, then asks for it. A code mailed less than a minute ago
/// is still the one to enter.
pub(crate) async fn sign_in_request(cx: &Cx, Form(form): Form<RequestForm>) -> Result<SeeOther> {
    let paths = paths(cx);
    let next = safe_next(Some(&form.next), paths.account_url(None));
    match request_sign_in_code(cx, &form.email).await? {
        Ok(()) => {}
        Err(refusal) if refusal.code() == "code_recently_sent" => {}
        Err(refusal) => {
            return Ok(see_other(
                paths.sign_in_url(Some(&next), Some(refusal.code())),
            ));
        }
    }
    set_sign_in_flow(
        cx,
        SignInFlow {
            email: form.email.trim().to_owned(),
            next,
        },
    )?;
    Ok(see_other(paths.sign_in_code_url(None, false)))
}

/// What the code form posts: the code, or `resend`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CodeForm {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub resend: String,
}

/// Signs in with the code (or mails a new one), then on to where the
/// shopper was going.
pub(crate) async fn sign_in_verify(cx: &Cx, Form(form): Form<CodeForm>) -> Result<SeeOther> {
    let paths = paths(cx);
    let Some(flow) = sign_in_flow(cx) else {
        return Ok(see_other(paths.sign_in_url(None, None)));
    };
    if !form.resend.is_empty() {
        let error = request_sign_in_code(cx, &flow.email).await?.err();
        return Ok(see_other(paths.sign_in_code_url(
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
        Err(refusal) => Ok(see_other(
            paths.sign_in_code_url(Some(refusal.code()), false),
        )),
    }
}

/// Ends the shopper's session, then home.
pub(crate) async fn sign_out(cx: &Cx) -> Result<SeeOther> {
    if let Some(token) = shopper_token(cx) {
        end_session(cx, &token).await?;
    }
    forget_shopper(cx);
    Ok(see_other(paths(cx).home_url()))
}

/// Signs the shopper out of every other browser, then back to the account.
/// This one stays signed in.
pub(crate) async fn sign_out_everywhere(cx: &Cx) -> Result<SeeOther> {
    let paths = paths(cx);
    let Some(token) = shopper_token(cx) else {
        return Ok(see_other(paths.account_url(None)));
    };
    match sign_out_everywhere_else(cx, &token).await? {
        Ok(()) => Ok(see_other(paths.account_done_url(SIGNED_OUT_ELSEWHERE))),
        Err(refusal) => {
            if refusal.is_signed_out() {
                forget_shopper(cx);
            }
            Ok(see_other(paths.account_url(Some(refusal.code()))))
        }
    }
}

/// Deletes the account; the browser then forgets the shopper too.
pub(crate) async fn delete(cx: &Cx) -> Result<SeeOther> {
    let paths = paths(cx);
    let Some(token) = shopper_token(cx) else {
        return Ok(see_other(paths.account_url(None)));
    };
    match delete_account(cx, &token).await? {
        Ok(()) => {}
        // The session was already over and nothing was deleted: sign in first.
        Err(refusal) if refusal.is_signed_out() => {
            forget_shopper(cx);
            let back = paths.account_delete_url();
            return Ok(see_other(
                paths.sign_in_url(Some(&back), Some(refusal.code())),
            ));
        }
        Err(refusal) => return Ok(see_other(paths.account_url(Some(refusal.code())))),
    }
    forget_shopper(cx);
    // It keeps the address and the e-mail typed at the last checkout.
    forget_checkout_draft(cx);
    Ok(see_other(paths.home_url()))
}

/// What the remove-address form posts.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RemoveAddressForm {
    pub address_id: String,
}

/// Forgets a saved address, then back to the account.
pub(crate) async fn address_remove(
    cx: &Cx,
    Form(form): Form<RemoveAddressForm>,
) -> Result<SeeOther> {
    let paths = paths(cx);
    let Some(token) = shopper_token(cx) else {
        return Ok(see_other(paths.account_url(None)));
    };
    let error = match remove_address(cx, &token, &form.address_id).await? {
        Ok(()) => None,
        Err(refusal) => {
            if refusal.is_signed_out() {
                forget_shopper(cx);
            }
            Some(refusal.code())
        }
    };
    Ok(see_other(paths.account_url(error)))
}

action!(sign_in_request_route = sign_in_request, Form<RequestForm>);
action!(sign_in_verify_route = sign_in_verify, Form<CodeForm>);
action!(sign_out_route = sign_out);
action!(sign_out_everywhere_route = sign_out_everywhere);
action!(delete_route = delete);
action!(
    address_remove_route = address_remove,
    Form<RemoveAddressForm>
);
