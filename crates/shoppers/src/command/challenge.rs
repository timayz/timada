//! The one-time codes mailed to prove an address. The domain issues a code
//! and remembers only the nonce it derives from under the host's key
//! ([`CodeVerifier`]); a subscription on every node mails the plaintext,
//! after taking the send on the stream so one node mails, and records how
//! it went. Guesses are counted on the stream, so nothing a client holds
//! can reset them, and the stream alone gives the code away to nobody.
//!
//! The address the code goes to is personal data: the stream keeps it sealed
//! under the address's own key, drawn by its first code and kept by the
//! caller ([`ShopperKeys`]). The account the address registers takes the
//! same key ([`Command::sign_in`](super::Command::sign_in)), so deleting the
//! account leaves the challenge's addresses unreadable too.

use evento::{Executor, ProjectionAggregate, WriteError};
use timada_core::keys::{self as key, Key as ShopperKey, Keys as ShopperKeys, new_key_id};
use timada_core::{Claim, Claimant, CodeVerifier, LEASE_SECS, MailOutcome, MailRecord, new_nonce};

use crate::aggregator::{
    AttemptFailed, CodeConsumed, CodeIssued, MailClaimed, MailDeferred, MailFailed, MailSent,
    MailUndeliverable,
};
use crate::error::ShopperError;
use crate::value_object::{CODE_MAX_ATTEMPTS, CODE_RESEND_AFTER_SECS, CODE_TTL_SECS, challenge_id};

use super::{ChallengeState, challenge_email_aad};

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
    /// The normalized address, opened.
    pub email: String,
    /// The primary subtag of the shopper's language, when known.
    pub language: Option<String>,
    pub code: String,
}

impl std::fmt::Debug for CodeMail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeMail")
            .field("email", &self.email)
            .field("language", &self.language)
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
    /// unix seconds, `language` the shopper's (a tag or a whole
    /// `Accept-Language` value) for the mail. Any address gets one: the
    /// first sign-in creates the account. Another code within
    /// [`CODE_RESEND_AFTER_SECS`] of a live one is refused, unless that one
    /// could not be delivered: then the shopper may try again at once. The
    /// first code of an address draws its key, which `keys` holds before the
    /// event names it; later ones reseal the address under the same key
    /// while it is readable. The mail's subscription writes on the same
    /// stream, so a write landing between the read and this one's own is no
    /// verdict: the request is decided again on the fresh state.
    pub async fn issue_code(
        &self,
        keys: &dyn ShopperKeys,
        codes: &dyn CodeVerifier,
        email: &str,
        language: Option<&str>,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<IssuedCode, ShopperError> {
        let email = normalize_email(email)?;
        let challenge_id = challenge_id(&email);
        let nonce = new_nonce();
        let code = codes.derive_code(&self.tenant, &challenge_id, &nonce);
        let language = language.and_then(timada_core::primary_language);
        // A key drawn by a try that lost the race serves the next one.
        let mut drawn = None;
        let written = async {
            let mut attempt = 0;
            loop {
                attempt += 1;
                let existing = self.load_challenge(&email).await?;
                if let Some(existing) = &existing
                    && !existing.consumed
                    && now < existing.issued_at + CODE_RESEND_AFTER_SECS
                    && !existing.mail.is_undeliverable()
                {
                    return Err(ShopperError::CodeRecentlySent);
                }
                let (key_id, key) = match &existing {
                    Some(existing) if !existing.key_id.is_empty() => {
                        match keys.get(&existing.key_id).await? {
                            Some(key) => (existing.key_id.clone(), key),
                            // The account was deleted: the address starts
                            // afresh, under a new key.
                            None => draw_key(keys, &mut drawn).await?,
                        }
                    }
                    _ => draw_key(keys, &mut drawn).await?,
                };
                let issued = CodeIssued {
                    key_id: key_id.clone(),
                    email: key::seal(
                        &key,
                        &challenge_email_aad(&challenge_id, &key_id),
                        email.as_bytes(),
                    )?,
                    nonce: nonce.clone(),
                    language: language.clone(),
                    issued_at: now,
                    expires_at: now + CODE_TTL_SECS,
                };
                let committed = match existing {
                    Some(existing) => {
                        existing
                            .write()?
                            .routing_key_opt(routing_key.clone())
                            .event(&issued)
                            .tenant(&self.tenant)
                            .commit(&self.executor)
                            .await
                    }
                    None => {
                        evento::append(&challenge_id)
                            .original_version(0)
                            .routing_key_opt(routing_key.clone())
                            .event(&issued)
                            .tenant(&self.tenant)
                            .commit(&self.executor)
                            .await
                    }
                };
                match committed {
                    Ok(_) => return Ok(key_id),
                    // The previous code's mail recorded, or another request
                    // landed: decide again on the fresh state, where a code
                    // issued meanwhile is in the mail.
                    Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
                    Err(err) => return Err(err.into()),
                }
            }
        }
        .await;
        if let Some((drawn_id, _)) = &drawn
            && written.as_ref().ok() != Some(drawn_id)
        {
            // Nothing names the key: drop it rather than keep it unused.
            let _ = keys.delete(drawn_id).await;
        }
        written.map(|_| IssuedCode { email, code })
    }

    /// Takes, for `claimant`, the sending of the code that `CodeIssued`
    /// at `issued_version` of the challenge `challenge_id` recorded: what to
    /// mail, with the address opened by its key from `keys`, or `None` when
    /// this node mails nothing (the outcome is recorded, another node is
    /// sending, the code was replaced or expired, or the key is destroyed).
    /// A node that holds the claim gets the mail again, to send it again
    /// after a failure worth a retry.
    pub async fn claim_code_mail(
        &self,
        keys: &dyn ShopperKeys,
        codes: &dyn CodeVerifier,
        challenge_id: &str,
        issued_version: u16,
        claimant: Claimant<'_>,
        routing_key: Option<String>,
    ) -> Result<Option<CodeMail>, ShopperError> {
        let Claimant { node, now } = claimant;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(challenge) = self.load_challenge_by_id(challenge_id).await? else {
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
                Claim::Held => return self.code_mail(keys, codes, &challenge).await,
                Claim::Take => {}
            }
            // Opened before the claim: a key gone leaves nothing to claim.
            let Some(mail) = self.code_mail(keys, codes, &challenge).await? else {
                return Ok(None);
            };
            let claimed = challenge
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&MailClaimed {
                    by: node.to_owned(),
                    until: now + LEASE_SECS,
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match claimed {
                Ok(_) => return Ok(Some(mail)),
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
    ) -> Result<(), ShopperError> {
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
    ) -> Result<(), ShopperError> {
        self.record_code_mail(
            challenge_id,
            issued_version,
            MailOutcome::Undeliverable(reason),
            routing_key,
        )
        .await
    }

    /// Records that `node`'s send failed at `now` (unix seconds) for `reason`, a
    /// reason worth a retry (the shop's server did not answer, a transient
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
    ) -> Result<(), ShopperError> {
        self.record_code_mail(
            challenge_id,
            issued_version,
            MailOutcome::NotSent { node, reason, now },
            routing_key,
        )
        .await
    }
}

/// The fresh key of an address that has none readable: the one `drawn`
/// holds, or one drawn now, which `keys` holds before an event names it.
async fn draw_key(
    keys: &dyn ShopperKeys,
    drawn: &mut Option<(String, ShopperKey)>,
) -> Result<(String, ShopperKey), ShopperError> {
    if drawn.is_none() {
        let key_id = new_key_id();
        let key = ShopperKey::generate();
        keys.put(&key_id, &key).await?;
        *drawn = Some((key_id, key));
    }
    Ok(drawn.clone().expect("drawn above"))
}

impl<E: Executor> super::Command<E> {
    /// The mail of a challenge's current code: `None` when its key is
    /// destroyed; an error when the key cannot be read yet.
    async fn code_mail(
        &self,
        keys: &dyn ShopperKeys,
        codes: &dyn CodeVerifier,
        challenge: &ChallengeState,
    ) -> Result<Option<CodeMail>, ShopperError> {
        let Some(key) = keys.get(&challenge.key_id).await? else {
            return Ok(None);
        };
        Ok(Some(CodeMail {
            email: challenge.open_email(&key)?,
            language: challenge.language.clone(),
            code: codes.derive_code(&self.tenant, &challenge.id, &challenge.nonce),
        }))
    }

    async fn record_code_mail(
        &self,
        challenge_id: &str,
        issued_version: u16,
        outcome: MailOutcome<'_>,
        routing_key: Option<String>,
    ) -> Result<(), ShopperError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(challenge) = self.load_challenge_by_id(challenge_id).await? else {
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
            match writer.tenant(&self.tenant).commit(&self.executor).await {
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
    pub(crate) async fn consume_code(
        &self,
        codes: &dyn CodeVerifier,
        email: &str,
        code: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), ShopperError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
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
            let matches = codes.verify(&self.tenant, &challenge.id, &challenge.nonce, code);
            let outcome = if matches {
                challenge
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&CodeConsumed)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
            } else {
                challenge
                    .write()?
                    .routing_key_opt(routing_key.clone())
                    .event(&AttemptFailed)
                    .tenant(&self.tenant)
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
                Err(ShopperError::TooManyAttempts)
            } else {
                Err(ShopperError::WrongCode)
            };
        }
    }
}
