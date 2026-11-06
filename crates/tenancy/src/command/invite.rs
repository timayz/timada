//! Invitations: a member with `members:manage` mails a link; whoever opens it
//! signed in joins with the role it carries. The stream is keyed by the hash
//! of the token, so the store never holds anything a link could be rebuilt
//! from.

use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{Accepted, Revoked, Sent};
use crate::error::TenancyError;
use crate::value_object::{
    INVITATION_TTL_SECS, PERMISSION_MEMBERS_MANAGE, generate_invitation_token, invitation_id,
    normalize_email,
};

use super::{InvitationState, TenantState};

/// Reload-and-retry budget for a seat granted while other writes of the
/// tenant race it.
const GRANT_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteMember {
    pub tenant_id: String,
    pub email: String,
    pub role: String,
}

/// A sent invitation with the token to put in the link. The plaintext token
/// exists only in this value (and the e-mail).
#[derive(Clone, PartialEq, Eq)]
pub struct SentInvitation {
    pub token: String,
    pub invitation: InvitationState,
}

impl std::fmt::Debug for SentInvitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SentInvitation")
            .field("token", &"******")
            .field("invitation", &self.invitation)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptInvitation {
    pub token: String,
    /// The signed-in user accepting.
    pub user_id: String,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records an invitation and returns the token for the link. Needs
    /// `members:manage`, and a role the actor may hand out (owners any,
    /// others a role whose permissions they hold, never `owner`:
    /// [`TenantState::may_hand_out`]). Mailing the link is the caller's job.
    pub async fn invite_member(
        &self,
        actor_user_id: &str,
        cmd: InviteMember,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<SentInvitation, TenancyError> {
        let email = normalize_email(&cmd.email)?;
        let tenant = self.require_active(&cmd.tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MEMBERS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if !tenant.has_role(&cmd.role) {
            return Err(TenancyError::UnknownRole);
        }
        if !tenant.may_hand_out(actor_user_id, &cmd.role) {
            return Err(TenancyError::Forbidden);
        }

        let token = generate_invitation_token();
        let id = invitation_id(&token);
        evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .requested_by(actor_user_id)
            .event(&Sent {
                tenant_id: tenant.id,
                email,
                role: cmd.role,
                invited_by: actor_user_id.to_owned(),
                sent_at: now,
                expires_at: now + INVITATION_TTL_SECS,
            })
            .commit(&self.0)
            .await
            .map_err(|err| match err {
                // 256 random bits collided: not a user's doing.
                WriteError::InvalidOriginalVersion => {
                    TenancyError::Other(anyhow::anyhow!("invitation token collision"))
                }
                err => err.into(),
            })?;
        let invitation = self
            .load_invitation(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("invitation {id} vanished after creation"))?;
        Ok(SentInvitation { token, invitation })
    }

    /// Joins the tenant the link belongs to. Marking the invitation accepted,
    /// at the version read, is the link's one use: a second person with a
    /// forwarded link, or a revocation, racing it loses there and is granted
    /// nothing. The seat is granted next, and by the invitation-grants
    /// subscription should that fail; an account already in the tenant keeps
    /// its seat. Returns the tenant joined.
    pub async fn accept_invitation(
        &self,
        cmd: AcceptInvitation,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<TenantState, TenancyError> {
        let invitation = self
            .load_invitation_by_token(&cmd.token)
            .await?
            .filter(|invitation| invitation.is_open_at(now))
            .ok_or(TenancyError::InvitationInvalid)?;
        let tenant_id = invitation.tenant_id.clone();
        let tenant = self.require_active(&tenant_id).await?;
        // Refused before the link is used, so it stays open.
        if !tenant.members.contains_key(&cmd.user_id) && !tenant.has_role(&invitation.role) {
            return Err(TenancyError::UnknownRole);
        }
        invitation
            .write()?
            .routing_key_opt(routing_key.clone())
            .requested_by(cmd.user_id.clone())
            .event(&Accepted {
                user_id: cmd.user_id,
                accepted_at: now,
            })
            .commit(&self.0)
            .await
            .map_err(|err| match err {
                // Used or revoked since it was read.
                WriteError::InvalidOriginalVersion => TenancyError::InvitationInvalid,
                err => err.into(),
            })?;
        self.grant_accepted_seat(&invitation.id, routing_key)
            .await?;
        self.require_tenant(&tenant_id).await
    }

    /// Withdraws an open invitation. Needs `members:manage` on its tenant.
    pub async fn revoke_invitation(
        &self,
        actor_user_id: &str,
        invitation_id: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let invitation = self
            .load_invitation(invitation_id)
            .await?
            .ok_or(TenancyError::InvitationInvalid)?;
        let tenant = self.require_active(&invitation.tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MEMBERS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if !invitation.is_open_at(now) {
            return Err(TenancyError::InvitationInvalid);
        }
        invitation
            .write()?
            .routing_key_opt(routing_key)
            .requested_by(actor_user_id)
            .event(&Revoked {
                revoked_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await
            .map_err(|err| match err {
                // Accepted since it was read.
                WriteError::InvalidOriginalVersion => TenancyError::InvitationInvalid,
                err => err.into(),
            })?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    /// Grants the seat an accepted invitation promised, once: not to an
    /// account already in the tenant, nor to one removed since it accepted
    /// (an old link is no way back in). A role removed in between refuses
    /// it. Retried against a fresh tenant while other writes race it; told
    /// twice, nothing more happens.
    pub(crate) async fn grant_accepted_seat(
        &self,
        invitation_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let Some(invitation) = self.load_invitation(invitation_id).await? else {
            return Ok(());
        };
        let (Some(user_id), Some(accepted_at_ms)) =
            (invitation.accepted_by.as_deref(), invitation.accepted_at_ms)
        else {
            return Ok(());
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let tenant = self.require_tenant(&invitation.tenant_id).await?;
            let removed_since = tenant
                .removed_members
                .get(user_id)
                .is_some_and(|removed_at_ms| *removed_at_ms >= accepted_at_ms);
            if tenant.members.contains_key(user_id) || removed_since {
                return Ok(());
            }
            match self
                .grant(
                    user_id,
                    tenant,
                    user_id,
                    &invitation.role,
                    routing_key.clone(),
                )
                .await
            {
                Err(TenancyError::Write(WriteError::InvalidOriginalVersion))
                    if attempt < GRANT_ATTEMPTS => {}
                outcome => return outcome,
            }
        }
    }
}
