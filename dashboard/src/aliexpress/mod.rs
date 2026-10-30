//! The platform's AliExpress app and the companies' accounts it acts for
//! (their sealed tokens), the client that calls AliExpress for a shop, and
//! the import that fills a draft from an item (see [`subscription`]).

pub mod forward;
pub mod subscription;
pub mod sync;

use std::sync::Arc;
use std::time::Duration;

use timada_aliexpress::{Client, DEFAULT_API_URL, DEFAULT_AUTHORIZE_URL, Market};
use timada_tenancy::{AliexpressSettings, CompanyState, TenantState};

use crate::cluster::Executor;
use crate::config::AliexpressConfig;
use crate::media::Media;
use crate::media_images::MediaImages;
use crate::seal::Sealer;

/// Who the dashboard says did what it does on its own.
pub const ALIEXPRESS_ACTOR: &str = timada_aliexpress::ALIEXPRESS_ACTOR;

pub struct Aliexpress {
    sealer: Option<Sealer>,
    /// The platform's app: its key and secret, once the operator gave one.
    app: Option<(String, String)>,
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
    /// How long a product goes before it is read again, in seconds.
    sync_every_secs: u64,
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
        let app = match (&config.app_key, &config.app_secret) {
            (Some(key), Some(secret)) if !key.trim().is_empty() && !secret.is_empty() => {
                Some((key.trim().to_owned(), secret.clone()))
            }
            (None, None) => None,
            _ => anyhow::bail!("aliexpress.app_key and aliexpress.app_secret go together"),
        };
        Ok(Arc::new(Self {
            sealer,
            app,
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
            sync_every_secs: config.sync_every_secs.max(1),
        }))
    }

    /// How long a product goes before it is read again, in seconds.
    pub fn sync_every_secs(&self) -> u64 {
        self.sync_every_secs
    }

    /// Whether companies can connect here: the platform has an app and can
    /// keep its tokens (`secrets.key` is set).
    pub fn enabled(&self) -> bool {
        self.sealer.is_some() && self.app.is_some()
    }

    fn sealer(&self) -> anyhow::Result<&Sealer> {
        self.sealer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no secret can be kept here (no [secrets] key)"))
    }

    pub fn seal(&self, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.sealer()?.seal(plain)
    }

    /// The platform's app key, for the address a company says yes on.
    fn app(&self) -> anyhow::Result<&(String, String)> {
        self.app
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no AliExpress app here (no [aliexpress] app_key)"))
    }

    /// The platform's client, acting for no account yet.
    pub fn app_client(&self) -> anyhow::Result<Client> {
        let (key, secret) = self.app()?;
        Ok(Client::new(
            self.http.clone(),
            &self.api_url,
            key.clone(),
            secret.clone(),
        ))
    }

    /// The client acting for the company's account; `None` before one said
    /// yes.
    pub fn client_of(&self, company: &CompanyState) -> anyhow::Result<Option<Client>> {
        let Some(authorization) = &company.aliexpress else {
            return Ok(None);
        };
        let token = self.sealer()?.open(&authorization.access_token_sealed)?;
        Ok(Some(self.app_client()?.with_session(token)))
    }

    /// The company a shop sells as, read from the platform store; `None`
    /// until the shop is linked to one.
    pub async fn company_of(&self, tenant: &TenantState) -> anyhow::Result<Option<CompanyState>> {
        let Some(link) = &tenant.company else {
            return Ok(None);
        };
        timada_tenancy::Command(self.platform.clone())
            .load_company(&link.company_id)
            .await
    }

    /// Whether the shop can call AliExpress: it is set up, and the company
    /// it sells as has an account connected.
    pub async fn connected(&self, tenant: &TenantState) -> anyhow::Result<bool> {
        if tenant.aliexpress.is_none() {
            return Ok(false);
        }
        Ok(self
            .company_of(tenant)
            .await?
            .is_some_and(|company| company.aliexpress.is_some()))
    }

    /// Renews the company's tokens when the access token lapses within a
    /// day, on one node per token (the one that claims the renewal in the
    /// platform store); the company as it then is. A renewal AliExpress
    /// refuses is logged: an owner connects again.
    pub async fn refresh_if_due(
        &self,
        company: CompanyState,
        now: u64,
    ) -> anyhow::Result<CompanyState> {
        let Some(authorization) = &company.aliexpress else {
            return Ok(company);
        };
        if authorization.expires_at > now + timada_aliexpress::REFRESH_MARGIN_SECS {
            return Ok(company);
        }
        if authorization.refresh_expires_at != 0 && authorization.refresh_expires_at <= now {
            tracing::warn!(company = %company.id, "AliExpress access lapsed: an owner has to connect again");
            return Ok(company);
        }
        if !timada_aliexpress::claim_token_refresh(
            &self.platform,
            ALIEXPRESS_ACTOR,
            &company.id,
            authorization.expires_at,
        )
        .await?
        {
            return Ok(company);
        }
        let refresh_token = self.sealer()?.open(&authorization.refresh_token_sealed)?;
        let tokens = match self.app_client()?.refresh_token(&refresh_token).await {
            Ok(tokens) => tokens,
            Err(err) => {
                tracing::warn!(company = %company.id, %err, "AliExpress refused to renew the access");
                return Ok(company);
            }
        };
        let account = if tokens.account.is_empty() {
            authorization.account.clone()
        } else {
            tokens.account.chars().take(120).collect()
        };
        let tenancy = timada_tenancy::Command(self.platform.clone());
        tenancy
            .refresh_aliexpress(
                ALIEXPRESS_ACTOR,
                &company.id,
                timada_tenancy::AuthorizeAliexpress {
                    access_token_sealed: self.seal(&tokens.access_token)?,
                    refresh_token_sealed: self.seal(&tokens.refresh_token)?,
                    expires_at: now + tokens.expires_in,
                    refresh_expires_at: if tokens.refresh_expires_in == 0 {
                        0
                    } else {
                        now + tokens.refresh_expires_in
                    },
                    account,
                },
            )
            .await?;
        tracing::info!(company = %company.id, "AliExpress access renewed");
        Ok(tenancy.load_company(&company.id).await?.unwrap_or(company))
    }

    /// Where an owner goes to connect the company's account.
    pub fn authorize_url(&self, redirect_uri: &str, state: &str) -> anyhow::Result<String> {
        let (key, _) = self.app()?;
        Ok(timada_aliexpress::authorize_url(
            &self.authorize_url,
            key,
            redirect_uri,
            state,
        ))
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
    /// price (by the item's rule, else the shop's) and availability, acting
    /// for `company`'s account. `store` is the store. A read another node
    /// made in between wins.
    pub async fn sync_link<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant: &TenantState,
        company: &CompanyState,
        link_id: &str,
    ) -> anyhow::Result<SyncResult> {
        let links = timada_aliexpress::Command::new(store.clone(), &tenant.id);
        let Some((link, version)) = links.load_link_versioned(link_id).await? else {
            return Ok(SyncResult::NotLinked);
        };
        if !link.is_linked() {
            return Ok(SyncResult::NotLinked);
        }
        let (Some(settings), Some(client)) = (&tenant.aliexpress, self.client_of(company)?) else {
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
        let catalog = timada_catalog::Command::new(store.clone(), &tenant.id);
        let pricing = link.pricing.as_ref().or(settings.pricing.as_ref());
        let outcome = timada_aliexpress::apply_sync(
            &catalog,
            ALIEXPRESS_ACTOR,
            &link,
            product.as_ref(),
            pricing,
        )
        .await?;
        for warning in &outcome.warnings {
            tracing::info!(tenant = %tenant.id, ae_product_id = %link.ae_product_id, %warning, "AliExpress read");
        }
        let found = (outcome.found != link.found).then_some(outcome.found);
        match links
            .record_sync(
                ALIEXPRESS_ACTOR,
                link_id,
                version,
                timada_aliexpress::SyncRead {
                    skus: outcome.skus,
                    offline: outcome.offline,
                    found,
                },
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

/// How adding combinations found went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddResult {
    /// This many are now variants; `warnings` says why the others are not.
    Added { added: usize, warnings: Vec<String> },
    /// Another read or add came in between: nothing recorded.
    Skipped,
    /// Nothing to add to: not linked, or the shop is not connected.
    NotLinked,
}

impl Aliexpress {
    /// Adds combinations a read found (`sku_ids`; empty: every one offered)
    /// as variants of the linked product, priced by the item's rule, else
    /// the shop's. `store` is the store.
    pub async fn add_combinations<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant: &TenantState,
        media: Arc<Media>,
        actor: &str,
        link_id: &str,
        sku_ids: &[String],
    ) -> anyhow::Result<AddResult> {
        let links = timada_aliexpress::Command::new(store.clone(), &tenant.id);
        let Some((link, version)) = links.load_link_versioned(link_id).await? else {
            return Ok(AddResult::NotLinked);
        };
        let Some(settings) = tenant.aliexpress.as_ref().filter(|_| link.is_linked()) else {
            return Ok(AddResult::NotLinked);
        };
        let catalog = timada_catalog::Command::new(store.clone(), &tenant.id);
        let images = self.images(media, &tenant.id);
        let outcome = timada_aliexpress::add_combinations(
            &catalog,
            actor,
            &link,
            sku_ids,
            link.pricing.as_ref().or(settings.pricing.as_ref()),
            &images,
        )
        .await?;
        for warning in &outcome.warnings {
            tracing::info!(tenant = %tenant.id, ae_product_id = %link.ae_product_id, %warning, "AliExpress combinations");
        }
        let added = outcome.skus.len();
        match links
            .record_combinations_added(actor, link_id, version, outcome.skus)
            .await
        {
            Ok(()) => Ok(AddResult::Added {
                added,
                warnings: outcome.warnings,
            }),
            Err(timada_aliexpress::AliexpressError::Write(
                evento::WriteError::InvalidOriginalVersion,
            )) => Ok(AddResult::Skipped),
            Err(err) => Err(err.into()),
        }
    }
}
