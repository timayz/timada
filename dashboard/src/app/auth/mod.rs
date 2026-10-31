//! Sign-in, sign-out, account management and the e-mail / OIDC / passkey
//! endpoints, all under `/auth`.

pub mod account;
pub mod email;
pub mod forgot_password;
pub mod login;
pub mod logout;
pub mod oauth2;
pub mod passkey;
pub mod password;
pub mod register;
pub mod sessions;

use timada_identity::IdentityError;
use topcoat::{context::Cx, router::href};

use crate::auth::cookies::{invite_flow, is_invite_fresh};
use crate::auth::now;

/// The query-string code the pages show for a domain error.
pub fn error_code(err: &IdentityError) -> &'static str {
    err.code().unwrap_or("failed")
}

#[derive(serde::Serialize)]
pub struct Flash<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'a str>,
}

pub fn login_url(cx: &Cx, error: Option<&str>) -> String {
    href!(login::login)
        .query(Flash {
            error,
            message: None,
        })
        .resolve(cx)
}

/// The sign-up form, with the address typed last time so it survives an
/// error.
pub fn register_url(cx: &Cx, error: Option<&str>, email: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Prefill<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<&'a str>,
    }
    href!(register::register)
        .query(Prefill { error, email })
        .resolve(cx)
}

pub fn forgot_password_url(cx: &Cx, error: Option<&str>) -> String {
    href!(forgot_password::forgot_password)
        .query(Flash {
            error,
            message: None,
        })
        .resolve(cx)
}

/// The page that asks for the mailed code.
pub fn verify_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(email::verify::verify)
        .query(Flash { error, message })
        .resolve(cx)
}

pub fn account_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(account::account)
        .query(Flash { error, message })
        .resolve(cx)
}

pub fn home_url(cx: &Cx) -> String {
    href!(crate::app::home).resolve(cx)
}

pub fn home_url_with_message(cx: &Cx, message: &str) -> String {
    href!(crate::app::home)
        .query(Flash {
            error: None,
            message: Some(message),
        })
        .resolve(cx)
}

/// Where a sign-in lands: the invitation the browser opened while signed
/// out, if one is waiting; else home, with a notice when the browser was
/// signed in as someone else before.
pub fn signed_in_url(cx: &Cx, label: &str, switched: bool) -> String {
    if let Some(url) = pending_invitation_url(cx) {
        return url;
    }
    if switched {
        home_url_with_message(cx, &format!("You are now signed in as {label}."))
    } else {
        home_url(cx)
    }
}

/// The invitation page a fresh `auth_invite` cookie points at. The cookie is
/// left in place: the invitation page removes it once the person accepts or
/// the link turns out invalid.
fn pending_invitation_url(cx: &Cx) -> Option<String> {
    let store = invite_flow(cx).parse().ok().flatten()?;
    let flow = store.get();
    if !is_invite_fresh(flow.issued_at, now()) {
        store.remove();
        return None;
    }
    Some(crate::app::invitations::token::invitation_url(
        cx,
        &flow.token,
        None,
    ))
}
