//! Membership: who belongs to a tenant and as what. Owners are special in two
//! ways: only an owner hands the `owner` role out or takes it back, and a
//! tenant never loses its last owner.

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{MemberGranted, MemberRevoked, MemberRoleChanged};
use crate::error::TenancyError;
use crate::value_object::{PERMISSION_MEMBERS_MANAGE, ROLE_OWNER};

use super::{Command, TenantState};

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Adds a user with a role. Needs `members:manage`; granting `owner`
    /// needs an owner; the role must exist.
    pub async fn grant_member(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        user_id: &str,
        role: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MEMBERS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if role == ROLE_OWNER && !tenant.is_owner(actor_user_id) {
            return Err(TenancyError::Forbidden);
        }
        self.grant(actor_user_id, tenant, user_id, role, routing_key)
            .await
    }

    /// Moves a member to another role. Needs `members:manage`; to or from
    /// `owner` needs an owner; the last owner stays. The same role is a no-op.
    pub async fn change_member_role(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        user_id: &str,
        role: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_MEMBERS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        let Some(current) = tenant.role_of(user_id) else {
            return Err(TenancyError::NotMember);
        };
        if !tenant.has_role(role) {
            return Err(TenancyError::UnknownRole);
        }
        if current == role {
            return Ok(());
        }
        let touches_owner = current == ROLE_OWNER || role == ROLE_OWNER;
        if touches_owner && !tenant.is_owner(actor_user_id) {
            return Err(TenancyError::Forbidden);
        }
        if current == ROLE_OWNER && tenant.owners() == 1 {
            return Err(TenancyError::LastOwner);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .requested_by(actor_user_id)
            .event(&MemberRoleChanged {
                user_id: user_id.to_owned(),
                role: role.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Removes a member. Needs `members:manage`, or `actor == user` (a member
    /// leaves); removing an owner needs an owner; the last owner stays.
    pub async fn revoke_member(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        let leaving = actor_user_id == user_id;
        if !leaving && !tenant.can(actor_user_id, PERMISSION_MEMBERS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        let Some(current) = tenant.role_of(user_id) else {
            return Err(TenancyError::NotMember);
        };
        if current == ROLE_OWNER {
            if !tenant.is_owner(actor_user_id) {
                return Err(TenancyError::Forbidden);
            }
            if tenant.owners() == 1 {
                return Err(TenancyError::LastOwner);
            }
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .requested_by(actor_user_id)
            .event(&MemberRevoked {
                user_id: user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}

impl<E: Executor> Command<E> {
    /// Appends the grant once the caller has settled who may do it (an actor
    /// with `members:manage`, or an accepted invitation, whose actor is the
    /// account joining).
    pub(crate) async fn grant(
        &self,
        actor_user_id: &str,
        tenant: TenantState,
        user_id: &str,
        role: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        if !tenant.has_role(role) {
            return Err(TenancyError::UnknownRole);
        }
        if tenant.members.contains_key(user_id) {
            return Err(TenancyError::AlreadyMember);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .requested_by(actor_user_id)
            .event(&MemberGranted {
                user_id: user_id.to_owned(),
                role: role.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
