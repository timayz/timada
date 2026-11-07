use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{PaymentsConfigured, PaymentsForgotten, PaymentsStopped};
use crate::error::TenancyError;
use crate::value_object::{FORGET_PAYMENTS_AFTER_SECS, PERMISSION_PAYMENTS_MANAGE};

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
    /// stay, sealed, so what is still held or owed is settled, until the
    /// merchant forgets them ([`forget_payments`](Self::forget_payments)).
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

    /// Forgets the keys of a tenant that stopped taking payments: the
    /// merchant's call, once the dashboard found nothing held or owed any
    /// more. Refused while payments are taken (`PaymentsNotStopped`) and
    /// until the stop is [`FORGET_PAYMENTS_AFTER_SECS`] old at `now` (unix
    /// seconds; `PaymentsStoppedRecently`): no card still holds money for a
    /// payment confirmed before it. Written at the version read, so keys
    /// saved again meanwhile win; nothing set up is a no-op. Needs
    /// `payments:manage`.
    pub async fn forget_payments(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_PAYMENTS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        let Some(payments) = &tenant.payments else {
            return Ok(());
        };
        let Some(stopped_at) = payments.stopped_at else {
            return Err(TenancyError::PaymentsNotStopped);
        };
        if now < stopped_at.saturating_add(FORGET_PAYMENTS_AFTER_SECS) {
            return Err(TenancyError::PaymentsStoppedRecently);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&PaymentsForgotten {
                forgotten_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
