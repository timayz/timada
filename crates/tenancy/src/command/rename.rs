use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Renamed, TimeZoneSet};
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

    /// Sets where the shop lives (an IANA name). Needs `tenant:manage`; the
    /// same zone is a no-op.
    pub async fn set_time_zone(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        time_zone: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let time_zone = timada_core::time_zone(time_zone).ok_or(TenancyError::InvalidTimeZone)?;
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_TENANT_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.time_zone.as_deref() == Some(time_zone.as_str()) {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&TimeZoneSet {
                time_zone,
                set_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
