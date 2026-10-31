//! Card payments, on each tenant's own Stripe account.
//!
//! A storefront asks to pay (`Cart::CheckoutRequested`); [`subscription`]
//! prices the cart from the store, opens a payment with Stripe under the
//! tenant's secret key and records it (`Cart::PaymentRequested`) for the
//! storefront's card form. Stripe tells the webhook (`app/stripe`) when the
//! payment went through, and only then does the order exist. Cancelling an
//! order gives the money back through Stripe first.
//!
//! The tenant's Stripe secrets are recorded sealed ([`Sealer`]) under the
//! operator's `secrets.key`; without a
//! `[payments]` section no tenant takes payments.

mod gateway;
mod setup;
pub mod signature;
mod subscription;
pub mod webhook;

use std::sync::Arc;

pub use gateway::*;
pub use setup::*;
pub use subscription::*;

use timada_tenancy::{CompanyState, TenantState};

use crate::cluster::Executor;
use crate::seal::Sealer;

/// Who the dashboard's own payment events say did them.
pub const DASHBOARD_ACTOR: &str = "timada-dashboard";

/// A tenant's Stripe account, opened.
#[derive(Clone)]
pub struct Account {
    pub publishable_key: String,
    pub secret_key: String,
    pub webhook_secret: String,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("publishable_key", &self.publishable_key)
            .finish_non_exhaustive()
    }
}

/// The payments service: the provider, and the key to the tenants' secrets.
pub struct Payments {
    sealer: Option<Sealer>,
    pub gateway: Arc<dyn PaymentGateway>,
    platform: Executor,
}

impl Payments {
    /// `sealer`: `None` turns payments off for every tenant (see
    /// [`payments_sealer`](crate::seal::payments_sealer)).
    pub fn new(
        sealer: Option<Sealer>,
        gateway: Arc<dyn PaymentGateway>,
        platform: Executor,
    ) -> Self {
        Self {
            sealer,
            gateway,
            platform,
        }
    }

    /// Whether the operator enabled payments on this node.
    pub fn enabled(&self) -> bool {
        self.sealer.is_some()
    }

    fn sealer(&self) -> anyhow::Result<&Sealer> {
        self.sealer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("payments are not enabled (no [payments] section)"))
    }

    pub fn seal(&self, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.sealer()?.seal(plain)
    }

    /// The tenant's Stripe account, opened; `None` when it set up none or
    /// payments are off here.
    pub fn account_of(&self, tenant: &TenantState) -> anyhow::Result<Option<Account>> {
        let (Some(settings), Some(sealer)) = (&tenant.payments, &self.sealer) else {
            return Ok(None);
        };
        Ok(Some(Account {
            publishable_key: settings.publishable_key.clone(),
            secret_key: sealer.open(&settings.secret_key_sealed)?,
            webhook_secret: sealer.open(&settings.webhook_secret_sealed)?,
        }))
    }

    /// The company a tenant sells as, once it has an identity.
    pub async fn company_of(&self, tenant: &TenantState) -> anyhow::Result<Option<CompanyState>> {
        timada_tenancy::Command(self.platform.clone())
            .load_company_of(tenant)
            .await
    }

    /// The tenant read from the platform store, unless it is suspended.
    pub async fn active_tenant(&self, tenant_id: &str) -> anyhow::Result<Option<TenantState>> {
        let tenant = timada_tenancy::Command(self.platform.clone())
            .load_tenant(tenant_id)
            .await?;
        Ok(tenant.filter(|tenant| !tenant.is_suspended()))
    }
}
