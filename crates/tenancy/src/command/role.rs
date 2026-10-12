//! Custom roles: a name, a label and the permissions it grants. `owner` is
//! not a defined role; it is implicit and cannot be edited.

use std::collections::BTreeSet;

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{RoleDefined, RoleRemoved, RoleUpdated};
use crate::error::TenancyError;
use crate::value_object::{
    PERMISSION_ROLES_MANAGE, normalize_permissions, normalize_role, normalize_role_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefineRole {
    /// The role name (slug rules; `owner` is refused).
    pub role: String,
    pub label: String,
    /// Entries of [`PERMISSIONS`](crate::PERMISSIONS); duplicates are fine.
    pub permissions: Vec<String>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Defines a new custom role. Needs `roles:manage`.
    pub async fn define_role(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        cmd: DefineRole,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let role = normalize_role(&cmd.role)?;
        let label = normalize_role_label(&cmd.label)?;
        let permissions = normalize_permissions(&cmd.permissions)?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ROLES_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.roles.contains_key(&role) {
            return Err(TenancyError::RoleExists);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&RoleDefined {
                role,
                label,
                permissions: permissions.into_iter().collect(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Replaces the label and the permission set of a custom role. Needs
    /// `roles:manage`; an unchanged role is a no-op.
    pub async fn update_role(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        role: &str,
        label: &str,
        permissions: Vec<String>,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let role = normalize_role(role)?;
        let label = normalize_role_label(label)?;
        let permissions: BTreeSet<String> = normalize_permissions(&permissions)?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ROLES_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        let Some(current) = tenant.roles.get(&role) else {
            return Err(TenancyError::UnknownRole);
        };
        if current.label == label && current.permissions == permissions {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&RoleUpdated {
                role,
                label,
                permissions: permissions.into_iter().collect(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Removes a custom role nobody holds. Needs `roles:manage`.
    pub async fn remove_role(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        role: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let role = normalize_role(role)?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ROLES_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if !tenant.roles.contains_key(&role) {
            return Err(TenancyError::UnknownRole);
        }
        if tenant.members_with_role(&role) > 0 {
            return Err(TenancyError::RoleInUse);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&RoleRemoved { role })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
