//! Taking a tenant offline and back. Platform administration: the web layer
//! restricts these to administrators (this crate cannot see identity's roles).

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Resumed, Suspended};
use crate::error::TenancyError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Suspends a tenant with a reason. Already suspended is a no-op (the
    /// first reason stands).
    pub async fn suspend_tenant(
        &self,
        tenant_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_tenant(tenant_id).await?;
        if tenant.is_suspended() {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&Suspended {
                reason: reason.trim().to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Puts a suspended tenant back online.
    pub async fn resume_tenant(
        &self,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_tenant(tenant_id).await?;
        if !tenant.is_suspended() {
            return Err(TenancyError::NotSuspended);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&Resumed)
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
