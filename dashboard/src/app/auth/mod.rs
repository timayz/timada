//! Sign-in, sign-out, account management and the OIDC / passkey endpoints,
//! all under `/auth`.

pub mod account;
pub mod login;
pub mod logout;
pub mod oauth2;
pub mod passkey;

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

pub fn account_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(account::account)
        .query(Flash { error, message })
        .resolve(cx)
}

pub fn home_url(cx: &Cx) -> String {
    href!(crate::app::home).resolve(cx)
}
