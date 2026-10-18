//! The shops' own AliExpress apps: their sealed secrets and tokens, the
//! client that calls AliExpress for a shop, and the import that fills a
//! draft from an item (see [`subscription`]).

pub mod subscription;

use std::sync::Arc;
use std::time::Duration;

use timada_aliexpress::{Client, DEFAULT_API_URL, DEFAULT_AUTHORIZE_URL, Market};
use timada_tenancy::{AliexpressSettings, TenantState};

use crate::cluster::Executor;
use crate::config::AliexpressConfig;
use crate::media::Media;
use crate::media_images::MediaImages;
use crate::seal::Sealer;

/// Who the dashboard says did what it does on its own.
pub const ALIEXPRESS_ACTOR: &str = timada_aliexpress::ALIEXPRESS_ACTOR;

pub struct Aliexpress {
    sealer: Option<Sealer>,
    api_url: String,
    authorize_url: String,
    /// Calls AliExpress.
    http: reqwest::Client,
    /// Fetches the pictures of the items imported: no redirects, so a
    /// public address cannot send it somewhere private.
    pictures: reqwest::Client,
    /// Whether pictures may come from this machine or a private network
    /// (`mail.allow_private_hosts`, for tests and local setups).
    allow_private_hosts: bool,
    platform: Executor,
}

impl Aliexpress {
    pub fn new(
        sealer: Option<Sealer>,
        config: &AliexpressConfig,
        allow_private_hosts: bool,
        platform: Executor,
    ) -> anyhow::Result<Arc<Self>> {
        // reqwest's rustls needs the process-wide crypto provider named.
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        Ok(Arc::new(Self {
            sealer,
            api_url: config
                .api_url
                .clone()
                .unwrap_or_else(|| DEFAULT_API_URL.to_owned()),
            authorize_url: config
                .authorize_url
                .clone()
                .unwrap_or_else(|| DEFAULT_AUTHORIZE_URL.to_owned()),
            http: reqwest::Client::builder()
                .user_agent("timada")
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            pictures: reqwest::Client::builder()
                .user_agent("timada")
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            allow_private_hosts,
            platform,
        }))
    }

    /// Whether this node can keep an app's secret (`secrets.key` is set).
    pub fn enabled(&self) -> bool {
        self.sealer.is_some()
    }

    fn sealer(&self) -> anyhow::Result<&Sealer> {
        self.sealer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no secret can be kept here (no [secrets] key)"))
    }

    pub fn seal(&self, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.sealer()?.seal(plain)
    }

    /// The app's client, acting for no account yet.
    pub fn app_client(&self, settings: &AliexpressSettings) -> anyhow::Result<Client> {
        let secret = self.sealer()?.open(&settings.app_secret_sealed)?;
        Ok(Client::new(
            self.http.clone(),
            &self.api_url,
            settings.app_key.clone(),
            secret,
        ))
    }

    /// The tenant's client, acting for its account; `None` before the shop
    /// set up its app and connected an account.
    pub fn client_of(&self, tenant: &TenantState) -> anyhow::Result<Option<Client>> {
        let Some(settings) = &tenant.aliexpress else {
            return Ok(None);
        };
        let Some(authorization) = &settings.authorization else {
            return Ok(None);
        };
        let token = self.sealer()?.open(&authorization.access_token_sealed)?;
        Ok(Some(self.app_client(settings)?.with_session(token)))
    }

    /// Where the merchant goes to connect their account.
    pub fn authorize_url(&self, app_key: &str, redirect_uri: &str, state: &str) -> String {
        timada_aliexpress::authorize_url(&self.authorize_url, app_key, redirect_uri, state)
    }

    /// What the shop buys in.
    pub fn market_of(settings: &AliexpressSettings) -> Market {
        Market {
            ship_to_country: settings.ship_to_country.clone(),
            currency: settings.currency.clone(),
            language: settings.language.clone(),
        }
    }

    /// Keeps a tenant's imported pictures.
    pub fn images(&self, media: Arc<Media>, tenant_id: &str) -> MediaImages {
        MediaImages::new(
            self.pictures.clone(),
            media,
            tenant_id,
            !self.allow_private_hosts,
        )
    }

    /// The tenant read from the platform store, unless it is suspended.
    pub async fn active_tenant(&self, tenant_id: &str) -> anyhow::Result<Option<TenantState>> {
        let tenant = timada_tenancy::Command(self.platform.clone())
            .load_tenant(tenant_id)
            .await?;
        Ok(tenant.filter(|tenant| !tenant.is_suspended()))
    }
}

/// How a read of one item went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncResult {
    /// Brought in step (`offline` once AliExpress no longer sells it).
    Synced { offline: bool, repriced: usize },
    /// Another node read it in between: nothing written.
    Skipped,
    /// Nothing to read: not linked, or the shop is not connected.
    NotLinked,
    /// AliExpress refused or did not answer; recorded on the link.
    Failed(String),
}

impl Aliexpress {
    /// Reads one linked item again and brings its product in step: stock,
    /// price (by the item's rule, else the shop's) and availability. `store`
    /// is the tenant's executor. A read another node made in between wins.
    pub async fn sync_link<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant: &TenantState,
        link_id: &str,
    ) -> anyhow::Result<SyncResult> {
        let links = timada_aliexpress::Command(store.clone());
        let Some((link, version)) = links.load_link_versioned(link_id).await? else {
            return Ok(SyncResult::NotLinked);
        };
        if !link.is_linked() {
            return Ok(SyncResult::NotLinked);
        }
        let (Some(settings), Some(client)) = (&tenant.aliexpress, self.client_of(tenant)?) else {
            return Ok(SyncResult::NotLinked);
        };
        let product = match client
            .product(&link.ae_product_id, &Self::market_of(settings))
            .await
        {
            Ok(product) => Some(product),
            Err(err) if err.gone => None,
            Err(err) => {
                let reason = if err.retryable {
                    "AliExpress did not answer; it is read again later.".to_owned()
                } else {
                    format!("AliExpress refused: {}", err.message)
                };
                links
                    .record_sync_failure(ALIEXPRESS_ACTOR, link_id, &reason)
                    .await?;
                return Ok(SyncResult::Failed(reason));
            }
        };
        let catalog = timada_catalog::Command(store.clone());
        let rule = link.pricing.or(settings.pricing);
        let outcome = timada_aliexpress::apply_sync(
            &catalog,
            ALIEXPRESS_ACTOR,
            &link,
            product.as_ref(),
            rule,
        )
        .await?;
        for warning in &outcome.warnings {
            tracing::info!(tenant = %tenant.id, ae_product_id = %link.ae_product_id, %warning, "AliExpress read");
        }
        match links
            .record_sync(
                ALIEXPRESS_ACTOR,
                link_id,
                version,
                outcome.skus,
                outcome.offline,
            )
            .await
        {
            Ok(()) => Ok(SyncResult::Synced {
                offline: outcome.offline,
                repriced: outcome.repriced,
            }),
            Err(timada_aliexpress::AliexpressError::Write(
                evento::WriteError::InvalidOriginalVersion,
            )) => Ok(SyncResult::Skipped),
            Err(err) => Err(err.into()),
        }
    }
}
