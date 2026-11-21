//! Invitations to create an account: root has a link mailed to an address,
//! and whoever opens it creates the account of that address, with a password
//! or through a provider that vouches for the same address. The stream is
//! keyed by the hash of the token and keeps only the nonce the token derives
//! from under a key the host holds ([`TokenDeriver`]), so the store never
//! holds anything a link could be rebuilt from without that key. A
//! subscription on every node mails the link, after taking the send on the
//! stream so one node mails, and records how it went.

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use timada_core::{
    Claim, Claimant, LEASE_SECS, MailOutcome, MailRecord, Sending, TokenDeriver, new_nonce,
};
use ulid::Ulid;

use crate::aggregator::{EmailRegistered, SignedInWithPassword};
use crate::error::IdentityError;
use crate::invitation::{
    Accepted, MailClaimed, MailDeferred, MailFailed, MailSent, MailUndeliverable, Revoked, Sent,
};
use crate::value_object::{
    ACCOUNT_INVITATION_TTL_SECS, LoginContext, ROOT_USER_ID, account_invitation_id, normalize_email,
};

use super::PasswordRegistration;

/// Reload-and-retry budget for a write on an invitation that raced another
/// (a revocation landing while the mail is claimed, or the mail's outcome
/// while the link is used, say).
const WRITE_ATTEMPTS: usize = 3;

#[evento::projection(id = id, name = "timada-identity/AccountInvitationState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct AccountInvitationState {
    pub id: String,
    /// Normalized address the link went to: the one it creates an account
    /// for.
    pub email: String,
    pub invited_by: String,
    /// Unix seconds.
    pub sent_at: u64,
    /// Unix seconds.
    pub expires_at: u64,
    /// The account the link created.
    pub accepted_by: Option<String>,
    pub accepted_at: Option<u64>,
    pub revoked: bool,
    /// What the link's token derives from under the host's key.
    pub nonce: Vec<u8>,
    /// Where the link's mail stands.
    pub mail: Sending,
}

impl AccountInvitationState {
    /// Whether the link still works at `now`: not used, not revoked, not
    /// expired.
    pub fn is_open_at(&self, now: u64) -> bool {
        self.accepted_by.is_none() && !self.revoked && now < self.expires_at
    }
}

pub fn account_invitation_projection<E: Executor>() -> Projection<E, AccountInvitationState> {
    Projection::new::<crate::invitation::AccountInvitation>()
        .handler(on_sent())
        .handler(on_accepted())
        .handler(on_revoked())
        .handler(on_mail_claimed())
        .handler(on_mail_sent())
        .handler(on_mail_undeliverable())
        .handler(on_mail_deferred())
        .handler(on_mail_failed())
        .strict()
}

#[evento::handler]
async fn on_sent(event: Event<Sent>, state: &mut AccountInvitationState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.email = event.data.email.to_owned();
    state.invited_by = event.data.invited_by.to_owned();
    state.sent_at = event.data.sent_at;
    state.expires_at = event.data.expires_at;
    state.nonce = event.data.nonce.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_accepted(
    event: Event<Accepted>,
    state: &mut AccountInvitationState,
) -> anyhow::Result<()> {
    state.accepted_by = Some(event.data.user_id.to_owned());
    state.accepted_at = Some(event.data.accepted_at);
    Ok(())
}

#[evento::handler]
async fn on_revoked(
    _event: Event<Revoked>,
    state: &mut AccountInvitationState,
) -> anyhow::Result<()> {
    state.revoked = true;
    Ok(())
}

#[evento::handler]
async fn on_mail_claimed(
    event: Event<MailClaimed>,
    state: &mut AccountInvitationState,
) -> anyhow::Result<()> {
    state.mail = state.mail.claimed(&event.data.by, event.data.until);
    Ok(())
}

#[evento::handler]
async fn on_mail_sent(
    _event: Event<MailSent>,
    state: &mut AccountInvitationState,
) -> anyhow::Result<()> {
    state.mail = Sending::Sent;
    Ok(())
}

#[evento::handler]
async fn on_mail_undeliverable(
    event: Event<MailUndeliverable>,
    state: &mut AccountInvitationState,
) -> anyhow::Result<()> {
    state.mail = Sending::Undeliverable {
        reason: event.data.reason.to_owned(),
    };
    Ok(())
}

#[evento::handler]
async fn on_mail_deferred(
    event: Event<MailDeferred>,
    state: &mut AccountInvitationState,
) -> anyhow::Result<()> {
    state.mail = Sending::Deferred {
        retry_at: event.data.retry_at,
        attempts: event.data.attempts,
        reason: event.data.reason.to_owned(),
    };
    Ok(())
}

#[evento::handler]
async fn on_mail_failed(
    event: Event<MailFailed>,
    state: &mut AccountInvitationState,
) -> anyhow::Result<()> {
    state.mail = Sending::Failed {
        reason: event.data.reason.to_owned(),
        attempts: event.data.attempts,
    };
    Ok(())
}

/// A sent invitation with the token to put in the link. The plaintext token
/// exists only in this value (and the e-mail).
#[derive(Clone, PartialEq, Eq)]
pub struct SentAccountInvitation {
    pub token: String,
    pub invitation: AccountInvitationState,
}

impl std::fmt::Debug for SentAccountInvitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SentAccountInvitation")
            .field("token", &"******")
            .field("invitation", &self.invitation)
            .finish()
    }
}

/// What the node that took the sending of an invitation puts in the mail.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountInvitationMail {
    pub invitation: AccountInvitationState,
    /// The token of the link.
    pub token: String,
}

impl std::fmt::Debug for AccountInvitationMail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountInvitationMail")
            .field("invitation", &self.invitation)
            .field("token", &"******")
            .finish()
    }
}

/// The invitation page's form: the link's token and the password chosen. The
/// web layer hashes the password (see [`crate::hash_password`]).
#[derive(Clone)]
pub struct RegisterInvited {
    pub token: String,
    pub password_hash: String,
    pub context: LoginContext,
}

impl std::fmt::Debug for RegisterInvited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterInvited").finish_non_exhaustive()
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records an invitation of `email` to create an account and returns
    /// the token for the link, derived from a fresh nonce by `tokens`. Root
    /// alone invites (`Forbidden` for anyone else), and only an address no
    /// account signs in with, root's own included (`AlreadyRegistered`). The
    /// link is mailed by the subscription that follows the stream
    /// ([`Self::claim_account_invitation_mail`]).
    pub async fn invite_account(
        &self,
        tokens: &dyn TokenDeriver,
        actor_user_id: &str,
        email: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<SentAccountInvitation, IdentityError> {
        if actor_user_id != ROOT_USER_ID {
            return Err(IdentityError::Forbidden);
        }
        let email = normalize_email(email)?;
        let root = self.load_user(ROOT_USER_ID).await?;
        if root.is_some_and(|root| root.account == email)
            || self.email_user(&email).await?.is_some()
        {
            return Err(IdentityError::AlreadyRegistered);
        }

        let nonce = new_nonce();
        let token = tokens.derive_token("", &nonce);
        let id = account_invitation_id(&token);
        evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .requested_by(actor_user_id)
            .event(&Sent {
                email,
                invited_by: actor_user_id.to_owned(),
                nonce,
                sent_at: now,
                expires_at: now + ACCOUNT_INVITATION_TTL_SECS,
            })
            .commit(&self.executor)
            .await
            .map_err(|err| match err {
                // 256 random bits collided: not a user's doing.
                WriteError::InvalidOriginalVersion => {
                    IdentityError::Other(anyhow::anyhow!("invitation token collision"))
                }
                err => err.into(),
            })?;
        let invitation = self
            .load_account_invitation(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("invitation {id} vanished after creation"))?;
        Ok(SentAccountInvitation { token, invitation })
    }

    /// Withdraws an open invitation: its link stops working. Root alone.
    pub async fn revoke_account_invitation(
        &self,
        actor_user_id: &str,
        invitation_id: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if actor_user_id != ROOT_USER_ID {
            return Err(IdentityError::Forbidden);
        }
        let mut attempt = 0;
        loop {
            attempt += 1;
            let invitation = self
                .load_account_invitation(invitation_id)
                .await?
                .filter(|invitation| invitation.is_open_at(now))
                .ok_or(IdentityError::InvitationInvalid)?;
            let revoked = invitation
                .write()?
                .routing_key_opt(routing_key.clone())
                .requested_by(actor_user_id)
                .event(&Revoked {
                    revoked_by: actor_user_id.to_owned(),
                })
                .commit(&self.executor)
                .await;
            match revoked {
                Ok(_) => return Ok(()),
                // The mail's outcome landed since the read, or the link was
                // used: decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Creates the account of the address the link was mailed to, with the
    /// password chosen on the invitation page. The link having reached the
    /// person is the proof of the address: no code is mailed. The new user
    /// is signed in.
    pub async fn register_invited(
        &self,
        cmd: RegisterInvited,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<PasswordRegistration, IdentityError> {
        let user_id = Ulid::generate().to_string();
        let invitation = self
            .use_account_invitation(&cmd.token, Proof::Link, &user_id, now, routing_key.clone())
            .await?;
        let email = invitation.email;

        evento::append(&user_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&EmailRegistered {
                email: email.clone(),
                label: email.clone(),
                password_hash: cmd.password_hash,
            })
            .event(&SignedInWithPassword {
                user_agent: cmd.context.user_agent,
                ip: cmd.context.ip,
            })
            .commit(&self.executor)
            .await?;

        match self.claim_email_index(&email, &user_id, routing_key).await {
            Ok(()) => {}
            Err(IdentityError::AlreadyRegistered) => {
                tracing::warn!(
                    orphan_user_id = %user_id,
                    "lost the registration race; the stub user is unreachable"
                );
                return Err(IdentityError::AlreadyRegistered);
            }
            Err(err) => return Err(err),
        }

        Ok(PasswordRegistration {
            user_id,
            label: email,
        })
    }

    /// Takes, for `claimant`, the sending of the invitation
    /// `invitation_id`'s link: what to mail, with the token derived again by
    /// `tokens`, or `None` when this node mails nothing (the outcome is
    /// recorded, another node is sending, or the link is no longer open). A
    /// node that holds the claim gets the mail again, to send it again after
    /// a failure worth a retry.
    pub async fn claim_account_invitation_mail(
        &self,
        tokens: &dyn TokenDeriver,
        invitation_id: &str,
        claimant: Claimant<'_>,
        routing_key: Option<String>,
    ) -> Result<Option<AccountInvitationMail>, IdentityError> {
        let Claimant { node, now } = claimant;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(invitation) = self.load_account_invitation(invitation_id).await? else {
                return Ok(None);
            };
            if !invitation.is_open_at(now) {
                return Ok(None);
            }
            match invitation.mail.claim(node, now) {
                Claim::Leave => return Ok(None),
                Claim::Held => return Ok(Some(invitation_mail(tokens, invitation))),
                Claim::Take => {}
            }
            let claimed = invitation
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&MailClaimed {
                    by: node.to_owned(),
                    until: now + LEASE_SECS,
                })
                .commit(&self.executor)
                .await;
            match claimed {
                Ok(_) => return Ok(Some(invitation_mail(tokens, invitation))),
                // Another node claimed it, or it was withdrawn: decide again
                // on the fresh state.
                Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Records that the invitation's link was handed to the mail server.
    /// Nothing is written once the outcome is recorded already.
    pub async fn record_account_invitation_mail_sent(
        &self,
        invitation_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.record_account_invitation_mail(invitation_id, MailOutcome::Sent, routing_key)
            .await
    }

    /// Records that `node`'s send failed at `now` (unix seconds) for
    /// `reason`, a reason worth a retry (the server did not answer, a
    /// transient refusal): deferred to the next try of the schedule
    /// ([`timada_core::RETRY_DELAYS_SECS`]), or given up on once none is
    /// left, which the users page shows. Any node may take the next try once
    /// it is due.
    pub async fn record_account_invitation_mail_not_sent(
        &self,
        invitation_id: &str,
        node: &str,
        reason: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.record_account_invitation_mail(
            invitation_id,
            MailOutcome::NotSent { node, reason, now },
            routing_key,
        )
        .await
    }

    /// Records that the mail server refused the address for good (or that it
    /// does not parse), with what it said: the users page shows it, and root
    /// sends a corrected one.
    pub async fn record_account_invitation_mail_undeliverable(
        &self,
        invitation_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.record_account_invitation_mail(
            invitation_id,
            MailOutcome::Undeliverable(reason),
            routing_key,
        )
        .await
    }
}

/// What says the invited address is the person's.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Proof<'a> {
    /// The link itself, which was mailed there.
    Link,
    /// A provider, for the normalized address it verified (`None`: it
    /// verified none).
    Provider(Option<&'a str>),
}

/// The mail of an invitation, its token derived again from the nonce.
fn invitation_mail(
    tokens: &dyn TokenDeriver,
    invitation: AccountInvitationState,
) -> AccountInvitationMail {
    let token = tokens.derive_token("", &invitation.nonce);
    AccountInvitationMail { invitation, token }
}

impl<E: Executor> super::Command<E> {
    /// An invitation by its stream id (what the pages list).
    pub async fn load_account_invitation(
        &self,
        invitation_id: &str,
    ) -> anyhow::Result<Option<AccountInvitationState>> {
        account_invitation_projection()
            .load(invitation_id)
            .execute(&self.executor)
            .await
    }

    /// The invitation a link token opens, if any.
    pub async fn load_account_invitation_by_token(
        &self,
        token: &str,
    ) -> anyhow::Result<Option<AccountInvitationState>> {
        self.load_account_invitation(&account_invitation_id(token))
            .await
    }

    /// Uses the link `token` opens for the account `user_id`, about to be
    /// created, and returns the invitation as it stood. Marking it accepted,
    /// at the version read, is the link's one use: a second browser with the
    /// same link, or a revocation, racing it loses there and creates nothing;
    /// the mail's outcome landing in between is read past, and the link
    /// checked again. A provider signing the person up must vouch for the
    /// invited address ([`Proof::Provider`]): the link serves no other
    /// (`InvitationForOtherAddress`, the link left open). The account is
    /// written after this returns; should that fail, the link is spent and
    /// root sends another.
    pub(crate) async fn use_account_invitation(
        &self,
        token: &str,
        proof: Proof<'_>,
        user_id: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<AccountInvitationState, IdentityError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let invitation = self
                .load_account_invitation_by_token(token)
                .await?
                .filter(|invitation| invitation.is_open_at(now))
                .ok_or(IdentityError::InvitationInvalid)?;
            if let Proof::Provider(vouched) = proof
                && vouched != Some(invitation.email.as_str())
            {
                return Err(IdentityError::InvitationForOtherAddress);
            }
            // Registered since the invitation left: nothing to create.
            if self.email_user(&invitation.email).await?.is_some() {
                return Err(IdentityError::AlreadyRegistered);
            }
            let accepted = invitation
                .write()?
                .routing_key_opt(routing_key.clone())
                .requested_by(user_id)
                .event(&Accepted {
                    user_id: user_id.to_owned(),
                    accepted_at: now,
                })
                .commit(&self.executor)
                .await;
            match accepted {
                Ok(_) => return Ok(invitation),
                // The mail's outcome landed since the read, or the link was
                // used or revoked: decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    async fn record_account_invitation_mail(
        &self,
        invitation_id: &str,
        outcome: MailOutcome<'_>,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(invitation) = self.load_account_invitation(invitation_id).await? else {
                return Ok(());
            };
            let record = invitation.mail.record(outcome, invitation.expires_at);
            if record == MailRecord::Nothing {
                return Ok(());
            }
            let mut writer = invitation.write()?;
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
                // Used or withdrawn meanwhile: write past it.
                Err(WriteError::InvalidOriginalVersion) if attempt < WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
}
