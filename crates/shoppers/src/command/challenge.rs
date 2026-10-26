//! The one-time codes mailed to prove an address. The domain issues a code
//! and remembers only its hash; the caller mails the plaintext. Guesses are
//! counted on the stream, so nothing a client holds can reset them.

use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{AttemptFailed, CodeConsumed, CodeIssued};
use crate::error::ShopperError;
use crate::value_object::{
    CODE_MAX_ATTEMPTS, CODE_RESEND_AFTER_SECS, CODE_TTL_SECS, challenge_id, code_hash,
    generate_code,
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

/// The canonical form of an address, as the orders context keeps it, so an
/// account finds the orders placed with it.
pub fn normalize_email(email: &str) -> Result<String, ShopperError> {
    timada_orders::normalize_email(email).map_err(|_| ShopperError::InvalidEmail)
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Issues a fresh code for `email`, replacing any earlier one; `now` is
    /// unix seconds. Any address gets one: the first sign-in creates the
    /// account. Another code within [`CODE_RESEND_AFTER_SECS`] of a live one
    /// is refused.
    pub async fn issue_code(
        &self,
        email: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<IssuedCode, ShopperError> {
        let email = normalize_email(email)?;
        let challenge_id = challenge_id(&email);
        let existing = self.load_challenge(&email).await?;
        if let Some(existing) = &existing
            && !existing.consumed
            && now < existing.issued_at + CODE_RESEND_AFTER_SECS
        {
            return Err(ShopperError::CodeRecentlySent);
        }

        let code = generate_code();
        let issued = CodeIssued {
            code_hash: code_hash(&challenge_id, &code),
            issued_at: now,
            expires_at: now + CODE_TTL_SECS,
        };
        match existing {
            Some(existing) => {
                existing
                    .write()?
                    .routing_key_opt(routing_key)
                    .event(&issued)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await?;
            }
            None => {
                let created = evento::append(&challenge_id)
                    .original_version(0)
                    .routing_key_opt(routing_key)
                    .event(&issued)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await;
                match created {
                    Ok(_) => {}
                    // Two first requests at once: the other one's code is
                    // in the mail.
                    Err(WriteError::InvalidOriginalVersion) => {
                        return Err(ShopperError::CodeRecentlySent);
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
    pub(crate) async fn consume_code(
        &self,
        email: &str,
        code: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), ShopperError> {
        let Some(challenge) = self.load_challenge(email).await? else {
            return Err(ShopperError::WrongCode);
        };
        if challenge.consumed {
            return Err(ShopperError::WrongCode);
        }
        if challenge.failed_attempts >= CODE_MAX_ATTEMPTS {
            return Err(ShopperError::TooManyAttempts);
        }
        if now >= challenge.expires_at {
            return Err(ShopperError::CodeExpired);
        }
        let matches = code_hash(&challenge.id, code.trim()) == challenge.code_hash;
        let outcome = if matches {
            challenge
                .write()?
                .routing_key_opt(routing_key)
                .event(&CodeConsumed)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
        } else {
            challenge
                .write()?
                .routing_key_opt(routing_key)
                .event(&AttemptFailed)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
        };
        match outcome {
            Ok(_) => {}
            // Two submissions raced: whichever won decided; this one did not.
            Err(WriteError::InvalidOriginalVersion) => return Err(ShopperError::WrongCode),
            Err(err) => return Err(err.into()),
        }
        if matches {
            Ok(())
        } else if challenge.failed_attempts + 1 >= CODE_MAX_ATTEMPTS {
            Err(ShopperError::TooManyAttempts)
        } else {
            Err(ShopperError::WrongCode)
        }
    }
}
