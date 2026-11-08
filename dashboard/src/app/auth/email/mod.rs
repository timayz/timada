//! The e-mail + password sign-in: `POST /auth/email/login` checks the
//! password and issues a code, `/auth/email/verify` takes the code back (for
//! a registration, a sign-in, a password reset or an address added to an
//! account), `/auth/email/resend` issues another one and `POST
//! /auth/email/add` starts adding an address from the account page. The step
//! in between travels in the `EmailFlow` cookie. The codes are mailed by the
//! subscription that follows the challenges ([`crate::code_mails`]), never
//! from the request.

pub mod add;
pub mod login;
pub mod resend;
pub mod verify;

use timada_identity::IdentityError;
use topcoat::{Result, context::Cx};

use crate::app::auth::error_code;
use crate::auth::{cookies::EmailFlowKind, identity, now};

/// What [`start_challenge`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    /// A fresh code is recorded and on its way.
    Sending,
    /// A code issued moments ago is still valid; none was issued.
    AlreadySent,
}

impl Started {
    /// The note for a form that starts a flow: the person still gets the
    /// page to type the code in.
    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::Sending => None,
            Self::AlreadySent => Some("We already sent you a code a moment ago; it still works."),
        }
    }
}

/// Issues a code for `email`; the subscription that follows the challenge
/// mails it. `Err` is the error code to show; infrastructure failures
/// propagate.
pub async fn start_challenge(
    cx: &Cx,
    email: &str,
    kind: EmailFlowKind,
) -> Result<Result<Started, &'static str>> {
    match identity(cx)
        .issue_email_code(email, kind.purpose(), now())
        .await
    {
        Ok(_) => Ok(Ok(Started::Sending)),
        Err(IdentityError::CodeRecentlySent) => Ok(Ok(Started::AlreadySent)),
        Err(err) if err.is_user_facing() => Ok(Err(error_code(&err))),
        Err(err) => Err(err.into()),
    }
}
