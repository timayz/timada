//! Invitations: a member with `members:manage` mails a link; whoever opens it
//! signed in joins with the role it carries. The stream is keyed by the hash
//! of the token, so the store never holds anything a link could be rebuilt
//! from.

use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{Accepted, Revoked, Sent};
use crate::error::TenancyError;
use crate::value_object::{
    INVITATION_TTL_SECS, PERMISSION_MEMBERS_MANAGE, ROLE_OWNER, generate_invitation_token,
    invitation_id, normalize_email,
};

use super::{InvitationState, TenantState};

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
    /// `members:manage`; inviting as `owner` needs an owner; the role must
    /// exist. Mailing the link is the caller's job.
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
        if cmd.role == ROLE_OWNER && !tenant.is_owner(actor_user_id) {
            return Err(TenancyError::Forbidden);
        }
        if !tenant.has_role(&cmd.role) {
            return Err(TenancyError::UnknownRole);
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

    /// Joins the tenant the link belongs to. The grant is appended first and
    /// the invitation marked second: a crash between the two leaves the link
    /// open, and accepting it again converges (an existing member is not
    /// granted twice). Returns the tenant joined.
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
        if !tenant.members.contains_key(&cmd.user_id) {
            self.grant(
                &cmd.user_id,
                tenant,
                &cmd.user_id,
                &invitation.role,
                routing_key.clone(),
            )
            .await?;
        }
        invitation
            .write()?
            .routing_key_opt(routing_key)
            .requested_by(cmd.user_id.clone())
            .event(&Accepted {
                user_id: cmd.user_id,
                accepted_at: now,
            })
            .commit(&self.0)
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
            .await?;
        Ok(())
    }
}
