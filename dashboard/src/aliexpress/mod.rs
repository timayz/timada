//! The platform's AliExpress app and the companies' accounts it acts for
//! (their sealed tokens), the client that calls AliExpress for a shop, and
//! the import that fills a draft from an item (see [`subscription`]).

pub mod budget;
pub mod forward;
pub mod payment_reads;
pub mod subscription;
pub mod sync;
#[cfg(test)]
mod tests;
pub mod webhook;

use std::sync::Arc;
use std::time::Duration;

use timada_aliexpress::{
    AliexpressError, Client, DEFAULT_API_URL, DEFAULT_AUTHORIZE_URL, LinkState, Market, Share,
    SyncRead,
};
use timada_tenancy::{AliexpressSettings, CompanyState, TenantState};

use self::budget::CallBudget;
use crate::cluster::Executor;
use crate::config::AliexpressConfig;
use crate::db::Db;
use crate::media::Media;
use crate::media_images::MediaImages;
use crate::seal::Sealer;

/// Who the dashboard says did what it does on its own.
pub const ALIEXPRESS_ACTOR: &str = timada_aliexpress::ALIEXPRESS_ACTOR;

/// How long a node may go unheard before the others share out what it reads.
pub const LIVE_WITHIN: Duration = Duration::from_secs(30);

/// How many times a read's record is tried when a request came in between.
const RECORD_ATTEMPTS: usize = 3;

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
    /// How recent a read must be for a payment to go by it, in seconds.
    read_fresh_secs: u64,
    /// How long an open order goes unread before it is read again, in seconds.
    order_read_every_secs: u64,
    /// What the app may spend, and what this node spent.
    budget: CallBudget,
    /// Which of the nodes up this one is, at the moment of asking.
    share: Arc<dyn Fn() -> Share + Send + Sync>,
}

impl Aliexpress {
    pub fn new(
        sealer: Option<Sealer>,
        config: &AliexpressConfig,
        allow_private_hosts: bool,
        platform: Executor,
        share: Arc<dyn Fn() -> Share + Send + Sync>,
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
            // The pictures' addresses are the supplier's, not the operator's:
            // public hosts only, checked at the address dialled.
            pictures: crate::net::public_client(allow_private_hosts)
                .timeout(Duration::from_secs(30))
                .build()?,
            allow_private_hosts,
            platform,
            sync_every_secs: config.sync_every_secs.max(1),
            read_fresh_secs: config.read_fresh_secs,
            order_read_every_secs: config.order_read_every_secs.max(1),
            budget: CallBudget::new(config.calls_per_day),
            share,
        }))
    }

    /// AliExpress's places in `country`, read at most once a day; `None`
    /// without a list.
    /// How long a product goes before it is read again, in seconds.
    pub fn sync_every_secs(&self) -> u64 {
        self.sync_every_secs
    }

    /// How recent a read must be for a payment to go by it, in seconds.
    pub fn read_fresh_secs(&self) -> u64 {
        self.read_fresh_secs
    }

    /// How long an open order goes unread before it is read again, in seconds.
    pub fn order_read_every_secs(&self) -> u64 {
        self.order_read_every_secs
    }

    /// What the app may spend, and what this node spent.
    pub fn budget(&self) -> &CallBudget {
        &self.budget
    }

    /// Which of the nodes up this one is: which imported items and open orders
    /// it reads.
    pub fn share(&self) -> Share {
        (self.share)()
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

    /// Seals a token of `company_id`'s account: it opens for that company
    /// alone.
    pub fn seal(&self, company_id: &str, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.sealer()?.seal_for(company_id, plain)
    }

    /// The platform's app key, for the address a company says yes on.
    pub(crate) fn app(&self) -> anyhow::Result<&(String, String)> {
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
        )
        .counting(self.budget.calls()))
    }

    /// The client acting for the company's account; `None` before one said
    /// yes.
    pub fn client_of(&self, company: &CompanyState) -> anyhow::Result<Option<Client>> {
        let Some(authorization) = &company.aliexpress else {
            return Ok(None);
        };
        let token = self
            .sealer()?
            .open_for(&company.id, &authorization.access_token_sealed)?;
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
        let refresh_token = self
            .sealer()?
            .open_for(&company.id, &authorization.refresh_token_sealed)?;
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
                    access_token_sealed: self.seal(&company.id, &tokens.access_token)?,
                    refresh_token_sealed: self.seal(&company.id, &tokens.refresh_token)?,
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

    /// A shop able to call AliExpress: active, set up, and selling as a
    /// company with an account, its access renewed first if due.
    pub async fn connected_shop(
        &self,
        tenant_id: &str,
        now: u64,
    ) -> anyhow::Result<Option<(TenantState, CompanyState)>> {
        let Some(tenant) = self.active_tenant(tenant_id).await? else {
            return Ok(None);
        };
        if tenant.aliexpress.is_none() {
            return Ok(None);
        }
        let Some(company) = self.company_of(&tenant).await? else {
            return Ok(None);
        };
        let company = self.refresh_if_due(company, now).await?;
        if company.aliexpress.is_none() {
            return Ok(None);
        }
        Ok(Some((tenant, company)))
    }
}

/// What a read is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode<'a> {
    /// The dashboard's own: it writes only what changed, and the link's
    /// stream stays short however often the item is read; a price the rule's
    /// stays close to, by the shop's tolerance, stays as it is.
    Routine,
    /// The member `by` waits for it: its outcome is recorded in their name,
    /// whatever that is, and every price follows its rule exactly.
    Asked { by: &'a str },
}

impl SyncMode<'_> {
    fn actor(&self) -> &str {
        match self {
            Self::Routine => ALIEXPRESS_ACTOR,
            Self::Asked { by } => by,
        }
    }

    fn records_everything(&self) -> bool {
        matches!(self, Self::Asked { .. })
    }
}

/// How a read of one item went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncResult {
    /// Brought in step (`offline` once AliExpress no longer sells it).
    Synced { offline: bool, repriced: usize },
    /// Another write to the link came in between: the catalog is in step, the
    /// link's record of the read is not written.
    Skipped,
    /// Nothing to read: not linked, or the shop is not connected.
    NotLinked,
    /// The product is archived or gone from the catalog: the link is over,
    /// nothing read.
    Unlinked,
    /// AliExpress refused or did not answer; recorded on the link.
    Failed(String),
}

impl Aliexpress {
    /// Reads one linked item again and brings its product in step: stock,
    /// price (by the item's rule, else the shop's) and availability, acting
    /// for `company`'s account. `store` is the store, `db` the read models,
    /// which keep when the read was. Another write to the link in between wins
    /// over this read's record. Nothing goes on the link's stream unless `mode`
    /// asks for it or the read found something it does not hold.
    pub async fn sync_link<E: evento::Executor + Clone>(
        &self,
        db: &Db,
        store: &E,
        tenant: &TenantState,
        company: &CompanyState,
        link_id: &str,
        mode: SyncMode<'_>,
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
        let catalog = timada_catalog::Command::new(store.clone(), &tenant.id);
        let retired = match link.product_id.as_deref() {
            Some(product_id) => catalog
                .load_product(product_id)
                .await?
                .is_none_or(|product| product.is_archived()),
            None => true,
        };
        if retired {
            links.unlink(ALIEXPRESS_ACTOR, link_id).await?;
            return Ok(SyncResult::Unlinked);
        }
        let product = match client
            .product_delivered(&link.ae_product_id, &Self::market_of(settings))
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
                if mode.records_everything() || link.last_error.as_deref() != Some(reason.as_str())
                {
                    links
                        .record_sync_failure(mode.actor(), link_id, &reason)
                        .await?;
                }
                timada_aliexpress::note_tried(db, &tenant.id, link_id, crate::auth::now()).await?;
                return Ok(SyncResult::Failed(reason));
            }
        };
        let pricing = link.pricing.as_ref().or(tenant.aliexpress_pricing.as_ref());
        let tolerance_bps = match mode {
            SyncMode::Routine => tenant.aliexpress_price_tolerance_bps,
            SyncMode::Asked { .. } => 0,
        };
        let outcome = timada_aliexpress::apply_sync(
            &catalog,
            ALIEXPRESS_ACTOR,
            &link,
            product.as_ref(),
            pricing,
            tolerance_bps,
        )
        .await?;
        for warning in &outcome.warnings {
            tracing::info!(tenant = %tenant.id, ae_product_id = %link.ae_product_id, %warning, "AliExpress read");
        }
        let synced = SyncResult::Synced {
            offline: outcome.offline,
            repriced: outcome.repriced,
        };
        let worth_a_record =
            mode.records_everything() || link.last_error.is_some() || outcome.differs_from(&link);
        let result = if worth_a_record {
            let found = outcome.found_differs_from(&link).then_some(outcome.found);
            let read = SyncRead {
                skus: outcome.skus,
                offline: outcome.offline,
                found,
            };
            if record_read(&links, mode.actor(), &link, version, read).await? {
                synced
            } else {
                SyncResult::Skipped
            }
        } else {
            synced
        };
        timada_aliexpress::note_read(db, &tenant.id, link_id, crate::auth::now()).await?;
        Ok(result)
    }
}

/// Records a read against the version it started from; whether it was. A
/// request that came in between changes nothing the read knew of, so the record
/// goes on top of it. Anything else wins: another read's record, a combination
/// added (the read does not know it), the link being over.
async fn record_read<E: evento::Executor>(
    links: &timada_aliexpress::Command<E>,
    actor: &str,
    started: &LinkState,
    mut version: u16,
    read: SyncRead,
) -> anyhow::Result<bool> {
    for _ in 0..RECORD_ATTEMPTS {
        match links
            .record_sync(actor, &started.id, version, read.clone())
            .await
        {
            Ok(()) => return Ok(true),
            Err(AliexpressError::Write(evento::WriteError::InvalidOriginalVersion)) => {}
            Err(err) => return Err(err.into()),
        }
        let Some((link, current)) = links.load_link_versioned(&started.id).await? else {
            return Ok(false);
        };
        if link.status != started.status
            || link.skus != started.skus
            || link.last_synced_at != started.last_synced_at
        {
            return Ok(false);
        }
        version = current;
    }
    Ok(false)
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
        if tenant.aliexpress.is_none() || !link.is_linked() {
            return Ok(AddResult::NotLinked);
        }
        let catalog = timada_catalog::Command::new(store.clone(), &tenant.id);
        let images = self.images(media, &tenant.id);
        let outcome = timada_aliexpress::add_combinations(
            &catalog,
            actor,
            &link,
            sku_ids,
            link.pricing.as_ref().or(tenant.aliexpress_pricing.as_ref()),
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
