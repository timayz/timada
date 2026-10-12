use evento::{Executor, ProjectionAggregate};

use crate::aggregator::Renamed;
use crate::error::TenancyError;
use crate::value_object::{PERMISSION_TENANT_MANAGE, normalize_name};

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Changes the display name. Needs `tenant:manage`; the same name is a
    /// no-op.
    pub async fn rename_tenant(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let name = normalize_name(name)?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_TENANT_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.name == name {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&Renamed { name })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
