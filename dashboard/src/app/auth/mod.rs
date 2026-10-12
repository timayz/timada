//! Sign-in, sign-out, account management and the e-mail / OIDC / passkey
//! endpoints, all under `/auth`.

pub mod account;
pub mod email;
pub mod forgot_password;
pub mod login;
pub mod logout;
pub mod oauth2;
pub mod passkey;
pub mod register;
pub mod sessions;

use timada_identity::IdentityError;
use topcoat::{context::Cx, router::href};

/// The query-string code the pages show for a domain error.
pub fn error_code(err: &IdentityError) -> &'static str {
    match err {
        IdentityError::NotRegistered => "not_registered",
        IdentityError::AlreadyRegistered => "already_registered",
        IdentityError::AlreadyLinkedToOtherUser => "already_linked",
        IdentityError::SessionMismatch => "session_changed",
        IdentityError::UnexpectedlyAuthorized => "signed_in",
        IdentityError::Unauthorized => "sign_in_required",
        IdentityError::LastSignInMethod => "last_method",
        IdentityError::TooManyPasskeys => "too_many_passkeys",
        IdentityError::UnknownCredential | IdentityError::UserNotFound => "not_registered",
        IdentityError::Forbidden => "not_allowed",
        IdentityError::InvalidCredentials => "invalid_credentials",
        IdentityError::InvalidCode => "invalid_code",
        IdentityError::CodeExpired => "code_expired",
        IdentityError::TooManyAttempts => "too_many_attempts",
        IdentityError::CodeRecentlySent => "code_recently_sent",
        IdentityError::InvalidEmail => "invalid_email",
        IdentityError::WeakPassword => "weak_password",
        IdentityError::InvalidState | IdentityError::Write(_) | IdentityError::Other(_) => "failed",
    }
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

/// Where a sign-in lands: home, with a notice when the browser was signed in
/// as someone else before.
pub fn signed_in_url(cx: &Cx, label: &str, switched: bool) -> String {
    if switched {
        home_url_with_message(cx, &format!("You are now signed in as {label}."))
    } else {
        home_url(cx)
    }
}
