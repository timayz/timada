use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{PaymentsConfigured, PaymentsRemoved};
use crate::error::TenancyError;
use crate::value_object::PERMISSION_PAYMENTS_MANAGE;

/// The merchant's account with the payment provider, its secrets sealed by
/// the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurePayments {
    pub provider: String,
    pub publishable_key: String,
    pub secret_key_sealed: Vec<u8>,
    pub webhook_secret_sealed: Vec<u8>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets (or replaces) how shoppers pay. Needs `payments:manage`.
    pub async fn configure_payments(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        cmd: ConfigurePayments,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let publishable_key = cmd.publishable_key.trim().to_owned();
        if !publishable_key.starts_with("pk_") || publishable_key.contains(char::is_whitespace) {
            return Err(TenancyError::InvalidPublishableKey);
        }
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_PAYMENTS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&PaymentsConfigured {
                provider: cmd.provider,
                publishable_key,
                secret_key_sealed: cmd.secret_key_sealed,
                webhook_secret_sealed: cmd.webhook_secret_sealed,
                configured_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Stops taking payments; nothing set up is a no-op. Needs
    /// `payments:manage`.
    pub async fn remove_payments(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_PAYMENTS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.payments.is_none() {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&PaymentsRemoved {
                removed_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
