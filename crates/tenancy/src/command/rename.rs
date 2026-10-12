use evento::{Executor, ProjectionAggregate};

use crate::aggregator::Renamed;
use crate::error::TenancyError;
use crate::value_object::normalize_name;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Changes the display name. Owners only; the same name is a no-op.
    pub async fn rename_tenant(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let name = normalize_name(name)?;
        let tenant = self.require_tenant(tenant_id).await?;
        if !tenant.is_owner(actor_user_id) {
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
