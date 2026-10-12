//! The e-mail + password sign-in: `POST /auth/email/login` checks the
//! password and mails a code, `/auth/email/verify` takes the code back (for a
//! registration, a sign-in, a password reset or an address added to an
//! account), `/auth/email/resend` mails another one and `POST /auth/email/add`
//! starts adding an address from the account page. The step in between
//! travels in the `EmailFlow` cookie.

pub mod add;
pub mod login;
pub mod resend;
pub mod verify;

use timada_identity::IdentityError;
use topcoat::{Result, context::Cx};

use crate::app::auth::error_code;
use crate::auth::{cookies::EmailFlowKind, identity, mail::send_code, now};

/// What [`start_challenge`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    /// A fresh code is in the mail.
    Sent,
    /// A code mailed moments ago is still valid; none was sent.
    AlreadySent,
}

impl Started {
    /// The note for a form that starts a flow: the person still gets the
    /// page to type the code in.
    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::Sent => None,
            Self::AlreadySent => Some("We already sent you a code a moment ago; it still works."),
        }
    }
}

/// Issues a code for `email` and mails it. `Err` is the error code to show;
/// infrastructure failures propagate.
pub async fn start_challenge(
    cx: &Cx,
    email: &str,
    kind: EmailFlowKind,
) -> Result<Result<Started, &'static str>> {
    match identity(cx)
        .issue_email_code(email, kind.purpose(), now())
        .await
    {
        Ok(issued) => {
            if let Err(err) = send_code(cx, &issued.email, kind, &issued.code).await {
                tracing::error!(%err, "cannot send the e-mail code");
                return Ok(Err("mail_failed"));
            }
            Ok(Ok(Started::Sent))
        }
        Err(IdentityError::CodeRecentlySent) => Ok(Ok(Started::AlreadySent)),
        Err(err) if err.is_user_facing() => Ok(Err(error_code(&err))),
        Err(err) => Err(err.into()),
    }
}
