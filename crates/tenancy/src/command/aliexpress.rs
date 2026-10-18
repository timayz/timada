use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{AliexpressAuthorized, AliexpressConfigured, AliexpressRemoved};
use crate::error::TenancyError;
use crate::value_object::{PERMISSION_ALIEXPRESS_MANAGE, PricingRule};

/// The merchant's AliExpress app, its secret sealed by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureAliexpress {
    pub app_key: String,
    pub app_secret_sealed: Vec<u8>,
    /// ISO 3166 alpha-2 (`FR`).
    pub ship_to_country: String,
    /// ISO 4217 (`EUR`).
    pub currency: String,
    /// ISO 639-1 (`fr`).
    pub language: String,
    pub pricing: Option<PricingRule>,
}

/// The tokens an AliExpress account gave the app, sealed by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeAliexpress {
    pub access_token_sealed: Vec<u8>,
    pub refresh_token_sealed: Vec<u8>,
    pub expires_at: u64,
    pub refresh_expires_at: u64,
    pub account: String,
}

fn is_code(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_alphabetic())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Sets (or replaces) the shop's AliExpress app. Another app key drops
    /// the account the old one acted for. Needs `aliexpress:manage`.
    pub async fn configure_aliexpress(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        cmd: ConfigureAliexpress,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let app_key = cmd.app_key.trim().to_owned();
        if !(1..=64).contains(&app_key.len()) || !app_key.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(TenancyError::InvalidAliexpressKey);
        }
        let country = cmd.ship_to_country.trim().to_ascii_uppercase();
        let currency = cmd.currency.trim().to_ascii_uppercase();
        let language = cmd.language.trim().to_ascii_lowercase();
        if !is_code(&country, 2) || !is_code(&currency, 3) || !is_code(&language, 2) {
            return Err(TenancyError::InvalidAliexpressMarket);
        }
        if cmd.pricing.is_some_and(|rule| !rule.is_valid()) {
            return Err(TenancyError::InvalidPricingRule);
        }
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ALIEXPRESS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&AliexpressConfigured {
                app_key,
                app_secret_sealed: cmd.app_secret_sealed,
                ship_to_country: country,
                currency,
                language,
                pricing: cmd.pricing,
                configured_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Records the tokens the account gave the app after the merchant
    /// connected it. Needs `aliexpress:manage`.
    pub async fn authorize_aliexpress(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        cmd: AuthorizeAliexpress,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ALIEXPRESS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        self.write_authorization(actor_user_id, tenant, cmd, routing_key)
            .await
    }

    /// Records tokens the host refreshed on its own: no member asked, so no
    /// permission applies; `actor` names the host's job.
    pub async fn refresh_aliexpress(
        &self,
        actor: &str,
        tenant_id: &str,
        cmd: AuthorizeAliexpress,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        self.write_authorization(actor, tenant, cmd, routing_key)
            .await
    }

    /// Stops importing from AliExpress; nothing set up is a no-op. Needs
    /// `aliexpress:manage`.
    pub async fn remove_aliexpress(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ALIEXPRESS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        if tenant.aliexpress.is_none() {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&AliexpressRemoved {
                removed_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    async fn write_authorization(
        &self,
        actor: &str,
        tenant: super::TenantState,
        cmd: AuthorizeAliexpress,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        if tenant.aliexpress.is_none() {
            return Err(TenancyError::AliexpressNotConfigured);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&AliexpressAuthorized {
                access_token_sealed: cmd.access_token_sealed,
                refresh_token_sealed: cmd.refresh_token_sealed,
                expires_at: cmd.expires_at,
                refresh_expires_at: cmd.refresh_expires_at,
                account: cmd.account,
                authorized_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
