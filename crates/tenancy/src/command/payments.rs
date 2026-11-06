use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{PaymentsConfigured, PaymentsForgotten, PaymentsStopped};
use crate::error::TenancyError;
use crate::value_object::PERMISSION_PAYMENTS_MANAGE;

use super::PaymentSettings;

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
    /// Sets (or replaces) how shoppers pay, and takes payments again after
    /// a stop. Needs `payments:manage`.
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

    /// Stops taking payments: no new payment is opened from now on. The keys
    /// stay, sealed, so what is still held or owed is settled; they are
    /// forgotten ([`forget_payments`](Self::forget_payments)) once nothing is.
    /// Nothing set up, or stopped already, is a no-op. Needs
    /// `payments:manage`.
    pub async fn stop_payments(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_PAYMENTS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant
            .payments
            .as_ref()
            .is_none_or(PaymentSettings::is_stopped)
        {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&PaymentsStopped {
                stopped_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Forgets the keys of a tenant that stopped taking payments, once the
    /// caller found nothing held or owed any more. Written at the version
    /// read, so keys saved again meanwhile win. Whether they were forgotten:
    /// not stopped (any more), or another node first, is `false`.
    pub async fn forget_payments(
        &self,
        actor: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, TenancyError> {
        let Some(tenant) = self.load_tenant(tenant_id).await? else {
            return Ok(false);
        };
        if !tenant
            .payments
            .as_ref()
            .is_some_and(PaymentSettings::is_stopped)
        {
            return Ok(false);
        }
        let written = tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&PaymentsForgotten {
                forgotten_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await;
        match written {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }
}
