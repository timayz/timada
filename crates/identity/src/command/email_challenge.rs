//! The one-time codes mailed to prove an address. The domain issues a code
//! and remembers only the nonce it derives from under the host's key
//! ([`CodeVerifier`]); a subscription on every node mails the plaintext,
//! after taking the send on the stream so one node mails, and records how
//! it went. Guesses are counted on the stream, so nothing a client holds
//! can reset them, and the stream alone gives the code away to nobody.

use evento::ProjectionAggregate;
use evento::{Executor, WriteError};
use timada_core::{Claim, Claimant, LEASE_SECS, MailOutcome, MailRecord, new_nonce};

use crate::aggregator::{
    AttemptFailed, CodeIssued, Consumed, MailClaimed, MailDeferred, MailFailed, MailSent,
    MailUndeliverable,
};
use crate::error::IdentityError;
use crate::value_object::{
    CODE_MAX_ATTEMPTS, CODE_RESEND_AFTER_SECS, CODE_TTL_SECS, ChallengePurpose, email_challenge_id,
    normalize_email,
};

use super::EmailChallengeState;

/// Reload-and-retry budget for a write on a challenge that raced another
/// (a guess landing while the mail is claimed, or the mail's outcome while a
/// code is checked or a new one issued, say).
const WRITE_ATTEMPTS: usize = 3;

/// A code issued. The plaintext exists only in this value and in the mail.
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

/// What the node that took the sending of a code puts in the mail.
#[derive(Clone, PartialEq, Eq)]
pub struct CodeMail {
    /// The normalized address.
    pub email: String,
    pub purpose: ChallengePurpose,
    pub code: String,
}

impl std::fmt::Debug for CodeMail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeMail")
            .field("email", &self.email)
            .field("purpose", &self.purpose)
            .field("code", &"******")
            .finish()
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Issues a fresh code for `email`, replacing any earlier one. A code for
    /// signing in or resetting a password needs a registered address; one for
    /// registering needs a free one. Another code within
    /// [`CODE_RESEND_AFTER_SECS`] of a live one is refused, unless that one
    /// could not be delivered: then the person may try again at once. The
    /// mail's subscription writes on the same stream, so a write landing
    /// between the read and this one's own is no verdict: the request is
    /// decided again on the fresh state.
    pub async fn issue_email_code(
        &self,
        email: &str,
        purpose: ChallengePurpose,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<IssuedCode, IdentityError> {
        let email = normalize_email(email)?;
        let registered = self.email_user(&email).await?.is_some();
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
        let nonce = new_nonce();
        let code = self.codes.derive_code("", &challenge_id, &nonce);
        let issued = CodeIssued {
            purpose,
            email: email.clone(),
            nonce,
            issued_at: now,
            expires_at: now + CODE_TTL_SECS,
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let existing = self.load_email_challenge(&email).await?;
            if let Some(existing) = &existing
                && !existing.consumed
                && now < existing.issued_at + CODE_RESEND_AFTER_SECS
                && !existing.mail.is_undeliverable()
            {
                return Err(IdentityError::CodeRecentlySent);
            }
            let written = match existing {
                Some(existing) => {
                    existing
                        .write()?
                        .routing_key_opt(routing_key.clone())
                        .event(&issued)
                        .commit(&self.executor)
                        .await
                }
                None => {
                    evento::append(&challenge_id)
                        .original_version(0)
                        .routing_key_opt(routing_key.clone())
                        .event(&issued)
                        .commit(&self.executor)
                        .await
                }
            };
            match written {
                Ok(_) => return Ok(IssuedCode { email, code }),
                // The previous code's mail recorded, or another request
                // landed: decide again on the fresh state, where a code
                // issued meanwhile is in the mail.
                Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Takes, for `claimant`, the sending of the code that `CodeIssued`
    /// at `issued_version` of the challenge `challenge_id` recorded: what to
    /// mail, or `None` when this node mails nothing (the outcome is
    /// recorded, another node is sending, the code was replaced or expired
    /// meanwhile). A node that holds the claim gets the mail again, to send
    /// it again after a failure worth a retry.
    pub async fn claim_code_mail(
        &self,
        challenge_id: &str,
        issued_version: u16,
        claimant: Claimant<'_>,
        routing_key: Option<String>,
    ) -> Result<Option<CodeMail>, IdentityError> {
        let Claimant { node, now } = claimant;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(challenge) = self.load_email_challenge_by_id(challenge_id).await? else {
                return Ok(None);
            };
            if challenge.issued_version != issued_version
                || challenge.consumed
                || now >= challenge.expires_at
            {
                return Ok(None);
            }
            match challenge.mail.claim(node, now) {
                Claim::Leave => return Ok(None),
                Claim::Held => return Ok(Some(self.code_mail(&challenge))),
                Claim::Take => {}
            }
            let claimed = challenge
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&MailClaimed {
                    by: node.to_owned(),
                    until: now + LEASE_SECS,
                })
                .commit(&self.executor)
                .await;
            match claimed {
                Ok(_) => return Ok(Some(self.code_mail(&challenge))),
                // Another node claimed it, or a guess landed: decide again
                // on the fresh state.
                Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Records that the code `CodeIssued` at `issued_version` recorded was
    /// handed to the mail server. Nothing is written once the code was
    /// replaced, or the outcome recorded already.
    pub async fn record_code_mail_sent(
        &self,
        challenge_id: &str,
        issued_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.record_code_mail(challenge_id, issued_version, MailOutcome::Sent, routing_key)
            .await
    }

    /// Records that the mail server refused the address for good (or that it
    /// does not parse), with what it said. The next code for the address may
    /// then be issued at once.
    pub async fn record_code_mail_undeliverable(
        &self,
        challenge_id: &str,
        issued_version: u16,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.record_code_mail(
            challenge_id,
            issued_version,
            MailOutcome::Undeliverable(reason),
            routing_key,
        )
        .await
    }

    /// Records that `node`'s send failed at `now` (unix seconds) for `reason`, a
    /// reason worth a retry (the server did not answer, a transient
    /// refusal): deferred to the next try of the schedule
    /// ([`timada_core::RETRY_DELAYS_SECS`]), or given up on once none is
    /// left. Any node may take the next try once it is due.
    pub async fn record_code_mail_not_sent(
        &self,
        challenge_id: &str,
        issued_version: u16,
        node: &str,
        reason: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.record_code_mail(
            challenge_id,
            issued_version,
            MailOutcome::NotSent { node, reason, now },
            routing_key,
        )
        .await
    }
}

impl<E: Executor> super::Command<E> {
    fn code_mail(&self, challenge: &EmailChallengeState) -> CodeMail {
        CodeMail {
            email: challenge.email.clone(),
            purpose: challenge.purpose,
            code: self.codes.derive_code("", &challenge.id, &challenge.nonce),
        }
    }

    async fn record_code_mail(
        &self,
        challenge_id: &str,
        issued_version: u16,
        outcome: MailOutcome<'_>,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(challenge) = self.load_email_challenge_by_id(challenge_id).await? else {
                return Ok(());
            };
            if challenge.issued_version != issued_version {
                return Ok(());
            }
            let record = challenge.mail.record(outcome, challenge.expires_at);
            if record == MailRecord::Nothing {
                return Ok(());
            }
            let mut writer = challenge.write()?;
            writer.routing_key_opt(routing_key.clone());
            match record {
                MailRecord::Sent => writer.event(&MailSent),
                MailRecord::Undeliverable(reason) => writer.event(&MailUndeliverable { reason }),
                MailRecord::Deferred {
                    retry_at,
                    attempts,
                    reason,
                } => writer.event(&MailDeferred {
                    retry_at,
                    attempts,
                    reason,
                }),
                MailRecord::Failed { reason, attempts } => {
                    writer.event(&MailFailed { reason, attempts })
                }
                MailRecord::Nothing => unreachable!("returned above"),
            };
            match writer.commit(&self.executor).await {
                Ok(_) => return Ok(()),
                // A guess landed meanwhile: write past it.
                Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Checks `code` against the live code of a normalized address and burns
    /// it. A wrong guess is recorded; the fifth kills the code. Callers act
    /// *after* this returns, so a code is never reusable. The mail's
    /// subscription writes on the same stream, so a write landing between
    /// the read and the check's own write is no verdict: the code is checked
    /// again on the fresh state, where a submission that burnt it first
    /// makes this one wrong.
    pub(crate) async fn consume_email_code(
        &self,
        email: &str,
        purpose: ChallengePurpose,
        code: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
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
            let matches = self.codes.verify("", &challenge.id, &challenge.nonce, code);
            let outcome = if matches {
                challenge
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&Consumed)
                    .commit(&self.executor)
                    .await
            } else {
                challenge
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&AttemptFailed)
                    .commit(&self.executor)
                    .await
            };
            match outcome {
                Ok(_) => {}
                // The mail's outcome, another submission or a new code landed
                // since the read: decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => continue,
                Err(err) => return Err(err.into()),
            }
            return if matches {
                Ok(())
            } else if challenge.failed_attempts + 1 >= CODE_MAX_ATTEMPTS {
                Err(IdentityError::TooManyAttempts)
            } else {
                Err(IdentityError::InvalidCode)
            };
        }
    }
}
