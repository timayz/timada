//! The one-time codes mailed to prove an address. The domain issues a code
//! and remembers only its MAC under the host's key ([`CodeVerifier`]); the
//! web layer mails the plaintext. Guesses are counted on the stream, so
//! nothing a client holds can reset them, and the stream alone gives the
//! code away to nobody.

use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{AttemptFailed, CodeIssued, Consumed};
use crate::error::IdentityError;
use crate::password::generate_code;
use crate::value_object::{
    CODE_MAX_ATTEMPTS, CODE_RESEND_AFTER_SECS, CODE_TTL_SECS, ChallengePurpose, email_challenge_id,
    normalize_email,
};

/// A code to mail. The plaintext exists only in this value.
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedCode {
    /// The normalized address the code is for.
    pub email: String,
    pub code: String,
}

impl std::fmt::Debug for IssuedCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedCode")
            .field("email", &self.email)
            .field("code", &"******")
            .finish()
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Issues a fresh code for `email`, replacing any earlier one. A code for
    /// signing in or resetting a password needs a registered address; one for
    /// registering needs a free one. Another code within
    /// [`CODE_RESEND_AFTER_SECS`] of a live one is refused.
    pub async fn issue_email_code(
        &self,
        email: &str,
        purpose: ChallengePurpose,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<IssuedCode, IdentityError> {
        let email = normalize_email(email)?;
        let owner = self.load_email_account(&email).await?;
        let registered = match owner {
            Some(owner) => self.load_user(&owner.user_id).await?.is_some(),
            None => false,
        };
        match purpose {
            ChallengePurpose::Register if registered => {
                return Err(IdentityError::AlreadyRegistered);
            }
            ChallengePurpose::SignIn | ChallengePurpose::ResetPassword if !registered => {
                return Err(IdentityError::NotRegistered);
            }
            _ => {}
        }

        let challenge_id = email_challenge_id(&email);
        let existing = self.load_email_challenge(&email).await?;
        if let Some(existing) = &existing
            && !existing.consumed
            && now < existing.issued_at + CODE_RESEND_AFTER_SECS
        {
            return Err(IdentityError::CodeRecentlySent);
        }

        let code = generate_code();
        let issued = CodeIssued {
            purpose,
            code_tag: self.codes.tag("", &challenge_id, &code),
            issued_at: now,
            expires_at: now + CODE_TTL_SECS,
        };
        match existing {
            Some(existing) => {
                existing
                    .write()?
                    .routing_key_opt(routing_key)
                    .event(&issued)
                    .commit(&self.executor)
                    .await?;
            }
            None => {
                let created = evento::append(&challenge_id)
                    .original_version(0)
                    .routing_key_opt(routing_key)
                    .event(&issued)
                    .commit(&self.executor)
                    .await;
                match created {
                    Ok(_) => {}
                    // Two first requests at once: the other one's code is
                    // in the mail.
                    Err(WriteError::InvalidOriginalVersion) => {
                        return Err(IdentityError::CodeRecentlySent);
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        Ok(IssuedCode { email, code })
    }
}

impl<E: Executor> super::Command<E> {
    /// Checks `code` against the live code of a normalized address and burns
    /// it. A wrong guess is recorded; the fifth kills the code. Callers act
    /// *after* this returns, so a code is never reusable.
    pub(crate) async fn consume_email_code(
        &self,
        email: &str,
        purpose: ChallengePurpose,
        code: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let Some(challenge) = self.load_email_challenge(email).await? else {
            return Err(IdentityError::InvalidCode);
        };
        if challenge.consumed || challenge.purpose != purpose {
            return Err(IdentityError::InvalidCode);
        }
        if challenge.failed_attempts >= CODE_MAX_ATTEMPTS {
            return Err(IdentityError::TooManyAttempts);
        }
        if now >= challenge.expires_at {
            return Err(IdentityError::CodeExpired);
        }
        let matches = self
            .codes
            .verify("", &challenge.id, code, &challenge.code_tag);
        let outcome = if matches {
            challenge
                .write()?
                .routing_key_opt(routing_key)
                .event(&Consumed)
                .commit(&self.executor)
                .await
        } else {
            challenge
                .write()?
                .routing_key_opt(routing_key)
                .event(&AttemptFailed)
                .commit(&self.executor)
                .await
        };
        match outcome {
            Ok(_) => {}
            // Two submissions raced: whichever won decided; this one did not.
            Err(WriteError::InvalidOriginalVersion) => return Err(IdentityError::InvalidCode),
            Err(err) => return Err(err.into()),
        }
        if matches {
            Ok(())
        } else if challenge.failed_attempts + 1 >= CODE_MAX_ATTEMPTS {
            Err(IdentityError::TooManyAttempts)
        } else {
            Err(IdentityError::InvalidCode)
        }
    }
}
