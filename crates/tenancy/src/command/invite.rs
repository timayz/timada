//! Invitations: a member with `members:manage` mails a link; whoever opens it
//! signed in joins with the role it carries. The stream is keyed by the hash
//! of the token, so the store never holds anything a link could be rebuilt
//! from.

use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{Accepted, Revoked, SeatGranted, SeatRefused, Sent};
use crate::error::TenancyError;
use crate::value_object::{
    INVITATION_TTL_SECS, PERMISSION_MEMBERS_MANAGE, generate_invitation_token, invitation_id,
    normalize_email,
};

use super::{InvitationState, Seat, TenantState};

/// Reload-and-retry budget for settling a seat while other writes of the
/// tenant or of the invitation race it.
const SEAT_ATTEMPTS: usize = 3;

/// The code a seat is refused with when the invitation's role was removed
/// before the grant: [`TenancyError::UnknownRole`]'s.
const ROLE_GONE: &str = "unknown_role";

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

/// What accepting an invitation came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    /// The tenant joined.
    pub tenant: TenantState,
    /// The invitation is the account's, but its seat could not be settled
    /// right away: it follows in a moment (the invitation-grants
    /// subscription).
    pub seat_pending: bool,
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
    /// nothing. The seat is settled next ([`Self::settle_seat`]); should that
    /// fail, the acceptance stands and the invitation-grants subscription
    /// settles it ([`Joined::seat_pending`]). A role removed between the two
    /// refuses the seat: `UnknownRole`.
    pub async fn accept_invitation(
        &self,
        cmd: AcceptInvitation,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<Joined, TenancyError> {
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
        match self.settle_seat(&invitation.id, routing_key).await {
            Ok(Some(Seat::Granted)) => Ok(Joined {
                tenant: self.require_tenant(&tenant_id).await?,
                seat_pending: false,
            }),
            Ok(Some(Seat::Refused(_))) => Err(TenancyError::UnknownRole),
            Ok(None) => Ok(Joined {
                tenant,
                seat_pending: true,
            }),
            Err(err) => {
                tracing::warn!(%err, invitation = %invitation.id, "the seat follows from the invitation-grants subscription");
                Ok(Joined {
                    tenant,
                    seat_pending: true,
                })
            }
        }
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
    /// Settles the seat an accepted invitation owes, once, and records the
    /// answer on the invitation (`SeatGranted`, `SeatRefused`) so every node
    /// follows it, however late it gets there. Recorded already, that answer
    /// stands. Otherwise an account already in the tenant, or removed since
    /// it accepted (an old link is no way back in), holds its seat; a role
    /// removed in between refuses it; anyone else is granted the seat, then
    /// the grant is recorded. Retried against fresh reads while other writes
    /// race it. `None` for an invitation not accepted.
    pub(crate) async fn settle_seat(
        &self,
        invitation_id: &str,
        routing_key: Option<String>,
    ) -> Result<Option<Seat>, TenancyError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(invitation) = self.load_invitation(invitation_id).await? else {
                return Ok(None);
            };
            if let Some(seat) = &invitation.seat {
                return Ok(Some(seat.clone()));
            }
            let (Some(user_id), Some(accepted_at_ms)) =
                (invitation.accepted_by.as_deref(), invitation.accepted_at_ms)
            else {
                return Ok(None);
            };
            let tenant = self.require_tenant(&invitation.tenant_id).await?;
            let removed_since = tenant
                .removed_members
                .get(user_id)
                .is_some_and(|removed_at_ms| *removed_at_ms >= accepted_at_ms);
            let seat = if tenant.members.contains_key(user_id) || removed_since {
                Seat::Granted
            } else if !tenant.has_role(&invitation.role) {
                Seat::Refused(ROLE_GONE.to_owned())
            } else {
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
                    Ok(()) => Seat::Granted,
                    Err(TenancyError::Write(WriteError::InvalidOriginalVersion))
                        if attempt < SEAT_ATTEMPTS =>
                    {
                        continue;
                    }
                    Err(err) => return Err(err),
                }
            };
            let mut record = invitation.write()?;
            record
                .routing_key_opt(routing_key.clone())
                .requested_by(user_id);
            match &seat {
                Seat::Granted => record.event(&SeatGranted),
                Seat::Refused(reason) => record.event(&SeatRefused {
                    reason: reason.clone(),
                }),
            };
            match record.commit(&self.0).await {
                Ok(_) => return Ok(Some(seat)),
                // Settled elsewhere meanwhile: read it back and follow it.
                Err(WriteError::InvalidOriginalVersion) if attempt < SEAT_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
}
