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
    AeError, AeProduct, AliexpressError, Client, DEFAULT_API_URL, DEFAULT_AUTHORIZE_URL,
    IMPORT_LOW_VALUE_EURO_CENTS, ImportCharge, LinkState, Market, RenewedTokens, SyncRead,
    TokenRenewal,
};
use timada_core::Share;
use timada_invoicing::rates::EURO;
use timada_tenancy::{AliexpressSettings, CompanyState, TenantState};

use self::budget::CallBudget;
use crate::cluster::{Among, ShareOf};
use crate::config::AliexpressConfig;
use crate::db::Db;
use crate::ecb::{EuroRates, RateAnswer};
use crate::media::Media;
use crate::media_images::MediaImages;
use crate::seal::Sealer;

/// Who the dashboard says did what it does on its own.
pub const ALIEXPRESS_ACTOR: &str = timada_aliexpress::ALIEXPRESS_ACTOR;

/// How many times a read's record is tried when a request came in between.
const RECORD_ATTEMPTS: usize = 3;

/// The platform's AliExpress app, one per node: what it may spend is this
/// node's to count. It holds no store: a shop and its company are read from
/// the store its caller reads, a page's through the cluster, the
/// background's from this node's file (#533).
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
    /// How long a product goes before it is read again, in seconds.
    sync_every_secs: u64,
    /// How recent a read must be for a payment to go by it, in seconds.
    read_fresh_secs: u64,
    /// How long an open order goes unread before it is read again, in seconds.
    order_read_every_secs: u64,
    /// How long a shipped parcel goes before AliExpress is asked again where
    /// it is, in seconds.
    parcel_read_every_secs: u64,
    /// What the app may spend, and what this node spent.
    budget: CallBudget,
    /// Which of the nodes up this one is, at the moment of asking.
    share: ShareOf,
    /// How much disk a tenant's pictures may take.
    quota: Arc<crate::media::Quota>,
    /// The reads after pushes that may run at once ([`PUSH_READS`]).
    push_reads: tokio::sync::Semaphore,
    /// The reads after pushes under way or waiting their turn.
    push_reads_running: std::sync::atomic::AtomicUsize,
    /// Told when the last of them ends.
    push_reads_idle: tokio::sync::Notify,
    /// What AliExpress adds for each kind of goods it ships into the EU from
    /// outside, in euro cents ([`Self::import_charge`]).
    import_duty_cents: u64,
    import_fee_cents: u64,
    /// The euro's rates, for a shop buying in another currency.
    euro_rates: Arc<dyn EuroRates>,
    /// How many orders a company's account places an hour; 0 for no pace
    /// ([`Self::paced_until`]).
    orders_per_hour: u32,
}

/// How many reads after pushes run at once on a node: a burst of pushes
/// waits its turn rather than all asking AliExpress together.
const PUSH_READS: usize = 4;

impl Aliexpress {
    pub fn new(
        sealer: Option<Sealer>,
        config: &AliexpressConfig,
        allow_private_hosts: bool,
        quota: Arc<crate::media::Quota>,
        share: ShareOf,
        euro_rates: Arc<dyn EuroRates>,
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
            sync_every_secs: config.sync_every_secs.max(1),
            read_fresh_secs: config.read_fresh_secs,
            order_read_every_secs: config.order_read_every_secs.max(1),
            parcel_read_every_secs: config.parcel_read_every_secs.max(1),
            budget: CallBudget::new(config.calls_per_day),
            quota,
            share,
            push_reads: tokio::sync::Semaphore::new(PUSH_READS),
            push_reads_running: std::sync::atomic::AtomicUsize::new(0),
            push_reads_idle: tokio::sync::Notify::new(),
            import_duty_cents: config.import_duty_cents,
            import_fee_cents: config.import_fee_cents,
            euro_rates,
            orders_per_hour: config.orders_per_hour,
        }))
    }

    /// Waits for the reads after pushes under way, or waiting their turn, to
    /// end.
    #[cfg(test)]
    pub(crate) async fn pushes_read(&self) {
        use std::sync::atomic::Ordering;
        loop {
            let idle = self.push_reads_idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.push_reads_running.load(Ordering::SeqCst) == 0 {
                return;
            }
            idle.await;
        }
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

    /// How long a shipped parcel goes before AliExpress is asked again where
    /// it is, in seconds.
    pub fn parcel_read_every_secs(&self) -> u64 {
        self.parcel_read_every_secs
    }

    /// What the app may spend, and what this node spent.
    pub fn budget(&self) -> &CallBudget {
        &self.budget
    }

    /// Which of the nodes up this one is: which imported items and open orders
    /// the sweeps over every shop's rows read here, and this node's part of
    /// the app's calls.
    pub fn share(&self) -> Share {
        (self.share)(Among::All)
    }

    /// Which of the nodes up that see `region`'s events this one is: which of
    /// the items and orders a saga is told about it reads. The sagas follow
    /// the node's region, so an event of `region` reaches the nodes serving
    /// it alone (one written without a region, the nodes without one), and
    /// its item must fall to one of them.
    pub fn share_in(&self, region: Option<&str>) -> Share {
        (self.share)(Among::Region(region))
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

    /// The company a shop sells as, read from the platform's streams in
    /// `store`; `None` until the shop is linked to one.
    pub async fn company_of<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant: &TenantState,
    ) -> anyhow::Result<Option<CompanyState>> {
        let Some(link) = &tenant.company else {
            return Ok(None);
        };
        timada_tenancy::Command(store.clone())
            .load_company(&link.company_id)
            .await
    }

    /// Whether the shop can call AliExpress: it is set up, and the company
    /// it sells as, read from `store`, has an account connected.
    pub async fn connected<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant: &TenantState,
    ) -> anyhow::Result<bool> {
        if tenant.aliexpress.is_none() {
            return Ok(false);
        }
        Ok(self
            .company_of(store, tenant)
            .await?
            .is_some_and(|company| company.aliexpress.is_some()))
    }

    /// Renews the company's tokens when the access token lapses within a
    /// quarter of its lifetime, a day at most
    /// ([`timada_aliexpress::refresh_margin`]: the lifetime is counted from
    /// when the company got the token), on one node per token (the one that
    /// claims the renewal in the
    /// platform's streams of `store`); the company as it then is, read back
    /// from `store`. The tokens AliExpress answers are recorded on the claim
    /// before the company hears of them, so a node that stops in between, or
    /// a store not taking writes for a while, leaves them for the next sweep
    /// to apply rather than losing them. A renewal AliExpress refuses is
    /// logged and tried again once the claim goes stale; one refused for good
    /// waits for an owner to connect again. So is one whose access lapses
    /// when the renewed one did, which renews nothing (#560): its tokens
    /// reach the company once, and the next ask uses them.
    pub async fn refresh_if_due<E: evento::Executor + Clone>(
        &self,
        store: &E,
        company: CompanyState,
        now: u64,
    ) -> anyhow::Result<CompanyState> {
        let Some(authorization) = &company.aliexpress else {
            return Ok(company);
        };
        let lifetime = authorization
            .expires_at
            .saturating_sub(authorization.authorized_at);
        if authorization.expires_at > now + timada_aliexpress::refresh_margin(lifetime) {
            return Ok(company);
        }
        if authorization.refresh_expires_at != 0 && authorization.refresh_expires_at <= now {
            tracing::warn!(company = %company.id, "AliExpress access lapsed: an owner has to connect again");
            return Ok(company);
        }
        let renewing = authorization.expires_at;
        let renewal = timada_aliexpress::claim_token_refresh(
            store,
            ALIEXPRESS_ACTOR,
            &company.id,
            renewing,
            now,
        )
        .await?;
        let tokens = match renewal {
            TokenRenewal::Held => return Ok(company),
            // A node that stopped, or could not write the company, after
            // AliExpress answered: its tokens are what the company is owed.
            TokenRenewal::Renewed(tokens) => tokens,
            TokenRenewal::Claimed { standing } => {
                // The latest refresh token AliExpress gave: an earlier
                // answer's that renewed nothing, else the company's.
                let refresh_token_sealed = standing
                    .as_ref()
                    .map_or(&authorization.refresh_token_sealed, |tokens| {
                        &tokens.refresh_token_sealed
                    });
                let refresh_token = self.sealer()?.open_for(&company.id, refresh_token_sealed)?;
                match self.app_client()?.refresh_token(&refresh_token).await {
                    Ok(tokens) => {
                        let renewed = RenewedTokens {
                            access_token_sealed: self.seal(&company.id, &tokens.access_token)?,
                            refresh_token_sealed: self.seal(&company.id, &tokens.refresh_token)?,
                            expires_at: now + tokens.expires_in,
                            refresh_expires_at: if tokens.refresh_expires_in == 0 {
                                0
                            } else {
                                now + tokens.refresh_expires_in
                            },
                            account: if tokens.account.is_empty() {
                                authorization.account.clone()
                            } else {
                                tokens.account.chars().take(120).collect()
                            },
                        };
                        // Recorded first: whatever happens to the company's
                        // write, the next sweep finds them on the claim.
                        if let Err(err) = with_write_retries(|| {
                            timada_aliexpress::record_token_renewal(
                                store,
                                ALIEXPRESS_ACTOR,
                                &company.id,
                                renewing,
                                &renewed,
                            )
                        })
                        .await
                        {
                            tracing::error!(company = %company.id, error = %err, "renewed AliExpress tokens not recorded on their claim");
                        }
                        if !renewed.renews(renewing) {
                            tracing::warn!(company = %company.id, expires_at = renewing, "AliExpress renewed the access without moving its expiry on: asked again once the claim goes stale");
                        }
                        renewed
                    }
                    Err(err) => {
                        tracing::warn!(company = %company.id, %err, "AliExpress refused to renew the access");
                        // What the earlier answer gave is the company's
                        // all the same.
                        match standing {
                            Some(tokens) => tokens,
                            None => return Ok(company),
                        }
                    }
                }
            }
        };
        let tenancy = timada_tenancy::Command(store.clone());
        let written = with_write_retries(|| async {
            tenancy
                .refresh_aliexpress(
                    ALIEXPRESS_ACTOR,
                    &company.id,
                    renewing,
                    timada_tenancy::AuthorizeAliexpress {
                        access_token_sealed: tokens.access_token_sealed.clone(),
                        refresh_token_sealed: tokens.refresh_token_sealed.clone(),
                        expires_at: tokens.expires_at,
                        refresh_expires_at: tokens.refresh_expires_at,
                        account: tokens.account.clone(),
                    },
                )
                .await
                .map_err(anyhow::Error::from)
        })
        .await?;
        // Applied before, or renewed meanwhile by another node or an owner:
        // nothing changed here.
        if !written {
            return Ok(company);
        }
        if tokens.renews(renewing) {
            tracing::info!(company = %company.id, "AliExpress access renewed");
        }
        Ok(tenancy.load_company(&company.id).await?.unwrap_or(company))
    }

    /// Until when (unix seconds) `company_id`'s account places no order, by
    /// its pace (`orders_per_hour`, spread across the hour: one at most every
    /// hour / `orders_per_hour`, and no more than that many in the last hour),
    /// as this node's read model knows what the account was asked to place;
    /// `None` when one may go at `now`. Nodes placing at the same moment may
    /// each let one through: a pace, not a lock (#470).
    pub async fn paced_until(
        &self,
        db: &Db,
        company_id: &str,
        now: u64,
    ) -> anyhow::Result<Option<u64>> {
        if self.orders_per_hour == 0 {
            return Ok(None);
        }
        const HOUR: u64 = 60 * 60;
        let spacing = HOUR / u64::from(self.orders_per_hour);
        let (asked, earliest, latest) =
            timada_aliexpress::asked_on_account(db, company_id, now.saturating_sub(HOUR) as i64)
                .await?;
        let mut until = latest.map_or(0, |latest| latest.max(0) as u64 + spacing);
        if asked >= self.orders_per_hour
            && let Some(earliest) = earliest
        {
            until = until.max(earliest.max(0) as u64 + HOUR);
        }
        Ok((until > now).then_some(until))
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

    /// What AliExpress adds for each kind of goods it ships to the market's
    /// country from `ships_from` (#557): the EU's charges on imports, when
    /// the market is in the EU and the goods come from outside it, or from
    /// where AliExpress does not say (most of what it sells ships from
    /// China). The duty carries the destination's standard VAT, as AliExpress
    /// charges it. In the market's currency, at the day's euro rate when that
    /// is not the euro; none when the rate cannot be had, which the log says.
    pub async fn import_charge(
        &self,
        market: &Market,
        ships_from: Option<&str>,
        now: u64,
    ) -> Option<ImportCharge> {
        let country = market.ship_to_country.as_str();
        if (self.import_duty_cents == 0 && self.import_fee_cents == 0)
            || !timada_invoicing::tax::in_eu(country)
            || ships_from.is_some_and(timada_invoicing::tax::in_eu)
        {
            return None;
        }
        let vat_bps = timada_invoicing::tax::rate_bps(country, "standard");
        let vat = (u128::from(self.import_duty_cents) * u128::from(vat_bps) + 5_000) / 10_000;
        let in_euros = ImportCharge {
            duty_cents: self.import_duty_cents + u64::try_from(vat).unwrap_or(u64::MAX),
            fee_cents: self.import_fee_cents,
            low_value_cents: IMPORT_LOW_VALUE_EURO_CENTS,
            vat_bps,
        };
        if market.currency.eq_ignore_ascii_case(EURO) {
            return Some(in_euros);
        }
        let day = timada_core::civil::format_date(now);
        let rate = match self.euro_rates.rate_on(&market.currency, &day).await {
            RateAnswer::Rate(rate) => rate,
            RateAnswer::NoSuchCurrency => {
                tracing::warn!(currency = %market.currency, country, "no euro rate for this currency: imports priced without the EU's charges");
                return None;
            }
            RateAnswer::Unavailable(reason) => {
                tracing::warn!(currency = %market.currency, country, %reason, "the euro rates could not be read: imports priced without the EU's charges for now");
                return None;
            }
        };
        Some(ImportCharge {
            duty_cents: rate.from_euro_cents(in_euros.duty_cents)?,
            fee_cents: rate.from_euro_cents(in_euros.fee_cents)?,
            low_value_cents: rate.from_euro_cents(in_euros.low_value_cents)?,
            vat_bps,
        })
    }

    /// One item as an import fills a draft from it: its combinations, each
    /// costing what delivering one to the market's country costs on top
    /// ([`Client::delivery`]), and what bringing it in costs
    /// ([`Self::import_charge`]), with the days delivery takes. An item no
    /// longer sold is left as read.
    pub async fn read_delivered(
        &self,
        client: &Client,
        ae_product_id: &str,
        market: &Market,
    ) -> Result<AeProduct, AeError> {
        let mut product = client.product(ae_product_id, market).await?;
        if let Some(delivery) = client.delivery(&product, market).await? {
            let charge = self
                .import_charge(market, delivery.ships_from.as_deref(), crate::auth::now())
                .await;
            product.add_delivery(&delivery, &market.currency, charge.as_ref());
        }
        Ok(product)
    }

    /// Keeps a tenant's imported pictures, within its quota.
    pub fn images(&self, media: Arc<Media>, tenant: &TenantState) -> MediaImages {
        MediaImages::new(
            self.pictures.clone(),
            media,
            tenant,
            !self.allow_private_hosts,
            self.quota.clone(),
        )
    }

    /// The tenant read from the platform's streams in `store`, unless it is
    /// suspended.
    pub async fn active_tenant<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant_id: &str,
    ) -> anyhow::Result<Option<TenantState>> {
        let tenant = timada_tenancy::Command(store.clone())
            .load_tenant(tenant_id)
            .await?;
        Ok(tenant.filter(|tenant| !tenant.is_suspended()))
    }

    /// A shop able to call AliExpress, read from `store`: active, set up,
    /// and selling as a company with an account, its access renewed first
    /// if due.
    pub async fn connected_shop<E: evento::Executor + Clone>(
        &self,
        store: &E,
        tenant_id: &str,
        now: u64,
    ) -> anyhow::Result<Option<(TenantState, CompanyState)>> {
        let Some(tenant) = self.active_tenant(store, tenant_id).await? else {
            return Ok(None);
        };
        if tenant.aliexpress.is_none() {
            return Ok(None);
        }
        let Some(company) = self.company_of(store, &tenant).await? else {
            return Ok(None);
        };
        let company = self.refresh_if_due(store, company, now).await?;
        if company.aliexpress.is_none() {
            return Ok(None);
        }
        Ok(Some((tenant, company)))
    }
}

/// What a read is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode<'a> {
    /// The dashboard's own, from the sweep or a cart: it writes only what
    /// changed, and the link's stream stays short however often the item is
    /// read; a price the rule's stays close to, by the shop's tolerance,
    /// stays as it is. What delivery costs is reused from the last time this
    /// node asked, while the combination it was asked for costs the same and
    /// that is less than [`DELIVERY_FRESH_SECS`] ago: one call for most
    /// reads, not two.
    Routine,
    /// Before a payment: as [`Routine`](Self::Routine), but delivery is asked
    /// again every time, since the shopper pays by it.
    Sale,
    /// The member `by` waits for it: its outcome is recorded in their name,
    /// whatever that is, every price follows its rule exactly, and delivery
    /// is asked again.
    Asked { by: &'a str },
}

/// How long a routine read goes by the delivery this node last asked for
/// an item, when the combination asked about still costs the same. Delivery
/// seldom moves on its own; when it does, the shop prices by the old fee
/// for a week at most, and every sale asks anew.
pub const DELIVERY_FRESH_SECS: u64 = 7 * 24 * 60 * 60;

impl SyncMode<'_> {
    fn actor(&self) -> &str {
        match self {
            Self::Routine | Self::Sale => ALIEXPRESS_ACTOR,
            Self::Asked { by } => by,
        }
    }

    fn reuses_delivery(&self) -> bool {
        matches!(self, Self::Routine)
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
        let product = match self
            .read_item(db, &client, &tenant.id, &link, settings, mode)
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
            SyncMode::Routine | SyncMode::Sale => tenant.aliexpress_price_tolerance_bps,
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

impl Aliexpress {
    /// Reads the item as `mode` wants it: its combinations, then what
    /// delivering one costs and takes, and where from ([`Client::delivery`]),
    /// which a routine read takes from this node's last answer instead
    /// ([`timada_aliexpress::delivery_read`]) while the combination it was
    /// asked for costs the same, the item is on sale and the answer is
    /// fresh. What is asked is kept for the next routine read. Each
    /// combination then costs what delivering it and bringing it in cost on
    /// top ([`Self::import_charge`]).
    async fn read_item(
        &self,
        db: &Db,
        client: &Client,
        tenant_id: &str,
        link: &LinkState,
        settings: &AliexpressSettings,
        mode: SyncMode<'_>,
    ) -> Result<AeProduct, AeError> {
        let market = Self::market_of(settings);
        let mut product = client.product(&link.ae_product_id, &market).await?;
        let now = crate::auth::now();
        let kept = if mode.reuses_delivery() && product.on_sale() {
            match timada_aliexpress::delivery_read(db, tenant_id, &link.id).await {
                Ok(read) => read.filter(|read| {
                    now.saturating_sub(read.read_at) < DELIVERY_FRESH_SECS
                        && product
                            .sku(&read.delivery.sku_id)
                            .is_some_and(|sku| sku.cost_cents == Some(read.delivery.sku_cost_cents))
                }),
                Err(err) => {
                    tracing::warn!(tenant = %tenant_id, link = %link.id, error = %err, "the delivery last read is not to be had: asking again");
                    None
                }
            }
        } else {
            None
        };
        let delivery = match kept {
            Some(read) => Some(read.delivery),
            None => {
                let delivery = client.delivery(&product, &market).await?;
                if let Some(delivery) = &delivery
                    && let Err(err) =
                        timada_aliexpress::note_delivery(db, tenant_id, &link.id, delivery, now)
                            .await
                {
                    tracing::warn!(tenant = %tenant_id, link = %link.id, error = %err, "the delivery read is not kept for the next read");
                }
                delivery
            }
        };
        if let Some(delivery) = delivery {
            let charge = self
                .import_charge(&market, delivery.ships_from.as_deref(), now)
                .await;
            product.add_delivery(&delivery, &market.currency, charge.as_ref());
        }
        Ok(product)
    }
}

/// Records a read against the version it started from; whether it was. A
/// write that came in between and changes nothing the read knew of (the
/// merchant's own rule, a combination ignored) leaves the record to go on top
/// of it. Anything else wins: another read's record, a combination added (the
/// read does not know it), the link being over.
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
        let images = self.images(media, tenant);
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

/// Runs `write` again, after a growing pause, while the store refuses it
/// for the moment or cannot tell whether it landed: some 16 seconds in all,
/// after which the error is the caller's.
async fn with_write_retries<T, F, Fut>(mut write: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut pause = Duration::from_millis(250);
    for _ in 0..6 {
        match write().await {
            Err(err) if is_transient(&err) => {
                tracing::debug!(error = %err, "store not taking the write yet, trying again");
                tokio::time::sleep(pause).await;
                pause *= 2;
            }
            done => return done,
        }
    }
    write().await
}

/// Whether the store refused a write only for the moment, or could not
/// tell whether it landed.
fn is_transient(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<evento::WriteError>(),
            Some(evento::WriteError::Unavailable(_) | evento::WriteError::Unknown(_))
        )
    })
}
