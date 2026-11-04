use evento::{Executor, ProjectionAggregate};

use crate::aggregator::company::{AliexpressAuthorized, AliexpressDisconnected};
use crate::aggregator::{
    AliexpressConfigured, AliexpressForwardingSet, AliexpressOrderPhoneSet,
    AliexpressPriceToleranceSet, AliexpressPricingSet, AliexpressRemoved,
};
use crate::command::CompanyState;
use crate::error::TenancyError;
use crate::value_object::{MAX_PRICE_TOLERANCE_BPS, PERMISSION_ALIEXPRESS_MANAGE, Pricing};

/// What the shop buys on AliExpress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureAliexpress {
    /// ISO 3166 alpha-2 (`FR`).
    pub ship_to_country: String,
    /// ISO 4217 (`EUR`).
    pub currency: String,
    /// ISO 639-1 (`fr`).
    pub language: String,
    /// How imports are priced, by cost band (`None`: by hand).
    pub pricing: Option<Pricing>,
    /// How far from a price, in hundredths of a percent, the rule's price may
    /// go before a read changes it.
    pub price_tolerance_bps: u32,
}

/// The tokens an AliExpress account gave the platform's app, sealed by the
/// host.
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
    /// Sets (or replaces) what the shop buys on AliExpress and how it prices
    /// it. Needs `aliexpress:manage`.
    pub async fn configure_aliexpress(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        cmd: ConfigureAliexpress,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let country = cmd.ship_to_country.trim().to_ascii_uppercase();
        let currency = cmd.currency.trim().to_ascii_uppercase();
        let language = cmd.language.trim().to_ascii_lowercase();
        if !is_code(&country, 2) || !is_code(&currency, 3) || !is_code(&language, 2) {
            return Err(TenancyError::InvalidAliexpressMarket);
        }
        if cmd
            .pricing
            .as_ref()
            .is_some_and(|pricing| !pricing.is_valid())
        {
            return Err(TenancyError::InvalidPricingRule);
        }
        if cmd.price_tolerance_bps > MAX_PRICE_TOLERANCE_BPS {
            return Err(TenancyError::InvalidPriceTolerance);
        }
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ALIEXPRESS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&AliexpressConfigured {
                ship_to_country: country,
                currency,
                language,
                configured_by: actor_user_id.to_owned(),
            })
            .event(&AliexpressPricingSet {
                pricing: cmd.pricing,
                set_by: actor_user_id.to_owned(),
            })
            .event(&AliexpressPriceToleranceSet {
                tolerance_bps: cmd.price_tolerance_bps,
                set_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Records the tokens the company's account gave the app after an
    /// owner connected it. Owners only.
    pub async fn authorize_aliexpress(
        &self,
        actor_user_id: &str,
        company_id: &str,
        cmd: AuthorizeAliexpress,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let company = self
            .require_company_owner(company_id, actor_user_id)
            .await?;
        self.write_authorization(actor_user_id, company, cmd, routing_key)
            .await
    }

    /// Records tokens the host refreshed on its own: no owner asked, so no
    /// ownership applies; `actor` names the host's job. A company with no
    /// account any more is left alone.
    pub async fn refresh_aliexpress(
        &self,
        actor: &str,
        company_id: &str,
        cmd: AuthorizeAliexpress,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let company = self.require_company(company_id).await?;
        if company.aliexpress.is_none() {
            return Ok(());
        }
        self.write_authorization(actor, company, cmd, routing_key)
            .await
    }

    /// Forgets the company's AliExpress account; none is a no-op. Owners
    /// only.
    pub async fn disconnect_aliexpress(
        &self,
        actor_user_id: &str,
        company_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let company = self
            .require_company_owner(company_id, actor_user_id)
            .await?;
        if company.aliexpress.is_none() {
            return Ok(());
        }
        company
            .write()?
            .routing_key_opt(routing_key)
            .event(&AliexpressDisconnected {
                disconnected_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// What happens to the shop's paid orders with AliExpress items; the
    /// same settings again are a no-op. Needs `aliexpress:manage`.
    pub async fn set_aliexpress_forwarding(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        auto_forward: bool,
        try_to_pay: bool,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ALIEXPRESS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        let Some(settings) = &tenant.aliexpress else {
            return Err(TenancyError::AliexpressNotConfigured);
        };
        if settings.auto_forward == auto_forward && settings.try_to_pay == try_to_pay {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&AliexpressForwardingSet {
                auto_forward,
                try_to_pay,
                set_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// The shop's phone for an order whose shopper gave none (AliExpress
    /// takes no order without one); blank clears it, the same again is a
    /// no-op. Needs `aliexpress:manage`.
    pub async fn set_aliexpress_order_phone(
        &self,
        actor_user_id: &str,
        tenant_id: &str,
        phone: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        let tenant = self.require_active(tenant_id).await?;
        if !tenant.can(actor_user_id, PERMISSION_ALIEXPRESS_MANAGE) {
            return Err(TenancyError::Forbidden);
        }
        let Some(settings) = &tenant.aliexpress else {
            return Err(TenancyError::AliexpressNotConfigured);
        };
        let phone = phone
            .map(str::trim)
            .filter(|phone| !phone.is_empty())
            .map(|phone| {
                let plain = phone
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '+' | '-' | '(' | ')' | '.'));
                let digits = phone.chars().filter(char::is_ascii_digit).count();
                if plain && (6..=32).contains(&phone.chars().count()) && digits >= 6 {
                    Ok(phone.to_owned())
                } else {
                    Err(TenancyError::InvalidPhone)
                }
            })
            .transpose()?;
        if settings.order_phone == phone {
            return Ok(());
        }
        tenant
            .write()?
            .routing_key_opt(routing_key)
            .event(&AliexpressOrderPhoneSet {
                phone,
                set_by: actor_user_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
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
        company: CompanyState,
        cmd: AuthorizeAliexpress,
        routing_key: Option<String>,
    ) -> Result<(), TenancyError> {
        company
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
