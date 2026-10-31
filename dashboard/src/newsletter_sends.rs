//! Sends the shops' newsletter campaigns: once a merchant sends one, every
//! node with the shop open mails its confirmed subscribers a batch at a time,
//! on a timer, from the shop. Each subscriber's mail is claimed first
//! (`Delivery`), so a subscriber gets a campaign once however many nodes
//! send it; a campaign with nobody left to mail is finished.
//!
//! A scheduled campaign goes out an edition per cycle of its schedule
//! ([`timada_newsletter::schedule`]): an edition opens when its first slot
//! comes somewhere on Earth, and at each look every time zone's subscribers
//! get the share their passed slots owe them, so 9:00 is 9:00 where each
//! subscriber lives (the shop's zone for those whose zone is unknown). The
//! look's interval bounds how close to a slot its mails go. Once its window
//! has closed everywhere, an edition is closed; a one-off one still owing
//! mails rolls on to the schedule's next cycle instead.
//!
//! A campaign goes through the mail server the shop picked for its
//! marketing and nothing else: a shop that picked none (or deleted it
//! mid-campaign) has its campaign held back, claimed by nobody, until it
//! picks one again. A server switched mid-campaign sends the next batches.

use std::sync::Arc;
use std::time::Duration;

use jiff::tz::TimeZone;
use sqlx::SqlitePool;
use timada_core::Keys;
use timada_newsletter::schedule::{parse_day, quota};
use timada_newsletter::{
    CampaignStatus, SubscriberRow, edition_progress, open_editions, scheduled_campaigns,
    sending_campaigns, undelivered_in_zone, undelivered_subscribers,
};
use timada_tenancy::NewProductVars;
use tokio::task::JoinHandle;
use topcoat::context::{AppContext, Cx};

use crate::auth::mail::{CampaignCopy, marketing_sender, send_campaign};
use crate::cluster::Executor;
use crate::key_vault::KeyVault;
use crate::newsletter::unsubscribe_link;
use crate::shop_mail::ShopSender;

/// Who the deliveries say mailed them.
pub const MAILER: &str = "timada-dashboard";

/// The most products a campaign shows as just in.
const NEW_PRODUCTS_SHOWN: i64 = 12;

/// How far back a campaign without an earlier edition looks for new
/// products, in seconds.
const NEW_PRODUCTS_LOOKBACK_SECS: i64 = 30 * 24 * 3600;

/// The shop's published products made after `since` (unix seconds), as a
/// campaign shows them: linked to their page on the storefront
/// (`{storefront}/p/{slug}`), with their first picture.
pub async fn new_products(
    read_pool: &SqlitePool,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    storefront: &str,
    since: i64,
) -> anyhow::Result<Vec<NewProductVars>> {
    let media = topcoat::context::app_context::<Arc<crate::media::Media>>(cx);
    Ok(
        timada_catalog::new_products(read_pool, &tenant.id, since, NEW_PRODUCTS_SHOWN)
            .await?
            .into_iter()
            .map(|product| NewProductVars {
                url: format!("{}/p/{}", storefront.trim_end_matches('/'), product.slug),
                price: product.price_label(),
                image_url: product
                    .thumbnail
                    .as_deref()
                    .map(|file| media.url(&tenant.id, file)),
                name: product.name,
            })
            .collect(),
    )
}

/// The sweep, with what it needs outside any request.
pub struct NewsletterSends {
    read_pool: SqlitePool,
    /// The store, every tenant's.
    store: Executor,
    mail: Arc<AppContext>,
    keys: Arc<KeyVault>,
    batch: u32,
}

impl NewsletterSends {
    pub fn new(
        read_pool: SqlitePool,
        store: Executor,
        mail: Arc<AppContext>,
        keys: Arc<KeyVault>,
        batch: u32,
    ) -> Arc<Self> {
        Arc::new(Self {
            read_pool,
            store,
            mail,
            keys,
            batch: batch.max(1),
        })
    }

    /// Sweeps every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(err) = self.sweep().await {
                    tracing::error!(error = %err, "newsletter campaigns not swept");
                }
            }
        })
    }

    /// Sends a batch of every campaign going out, opens the editions of
    /// scheduled campaigns that are due and sends what their slots owe. A
    /// campaign that fails is logged and the others still go.
    pub async fn sweep(&self) -> anyhow::Result<()> {
        let now = crate::auth::now() as i64;
        for (tenant_id, campaign_id) in sending_campaigns(&self.read_pool).await? {
            if let Err(err) = self.sweep_campaign(&tenant_id, &campaign_id).await {
                tracing::error!(tenant = %tenant_id, campaign = %campaign_id, error = %err, "newsletter batch not sent");
            }
        }
        for (tenant_id, campaign_id) in scheduled_campaigns(&self.read_pool).await? {
            let Some(tenant) = self.active_tenant(&tenant_id).await? else {
                continue;
            };
            if let Err(err) = open_due_editions(&self.store, &tenant, &campaign_id, now).await {
                tracing::error!(tenant = %tenant_id, campaign = %campaign_id, error = %err, "newsletter editions not opened");
            }
        }
        for (tenant_id, edition_id) in open_editions(&self.read_pool).await? {
            let Some(tenant) = self.active_tenant(&tenant_id).await? else {
                continue;
            };
            let cx = Cx::new(self.mail.clone());
            if let Err(err) = send_edition_batch(
                &self.store,
                &self.read_pool,
                &cx,
                &tenant,
                &*self.keys.for_tenant(&tenant_id),
                &edition_id,
                self.batch,
                now,
            )
            .await
            {
                tracing::error!(tenant = %tenant_id, edition = %edition_id, error = %err, "newsletter edition batch not sent");
            }
        }
        Ok(())
    }

    /// The tenant, unless it is gone or suspended.
    async fn active_tenant(
        &self,
        tenant_id: &str,
    ) -> anyhow::Result<Option<timada_tenancy::TenantState>> {
        Ok(timada_tenancy::Command(self.store.clone())
            .load_tenant(tenant_id)
            .await?
            .filter(|tenant| !tenant.is_suspended()))
    }

    async fn sweep_campaign(&self, tenant_id: &str, campaign_id: &str) -> anyhow::Result<()> {
        let Some(tenant) = self.active_tenant(tenant_id).await? else {
            return Ok(());
        };
        let cx = Cx::new(self.mail.clone());
        send_batch(
            &self.store,
            &self.read_pool,
            &cx,
            &tenant,
            &*self.keys.for_tenant(tenant_id),
            campaign_id,
            self.batch,
        )
        .await?;
        Ok(())
    }
}

/// Mails up to `batch` of the campaign's subscribers not mailed yet, then
/// finishes the campaign when nobody is left. Returns how many were mailed.
pub async fn send_batch<E: evento::Executor + Clone>(
    store: &E,
    read_pool: &SqlitePool,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    keys: &dyn Keys,
    campaign_id: &str,
    batch: u32,
) -> anyhow::Result<usize> {
    let newsletter = timada_newsletter::Command::new(store.clone(), &tenant.id);
    let Some(campaign) = newsletter.load_campaign(campaign_id).await? else {
        return Ok(0);
    };
    if campaign.status != CampaignStatus::Sending {
        return Ok(0);
    }
    // Without the storefront there is no unsubscribe link to put in the
    // mails: nothing goes out until the merchant says where it lives.
    let Some(storefront) = timada_orders::Command::new(store.clone(), &tenant.id)
        .storefront_address()
        .await?
    else {
        tracing::warn!(tenant = %tenant.id, campaign = %campaign.id, "newsletter held back: the shop has no storefront address");
        return Ok(0);
    };
    // Marketing goes through the server the shop picked for it, never the
    // platform's: without one nothing is claimed, so the mails wait for it.
    let Some(sender) = marketing_sender(cx, tenant).await? else {
        tracing::warn!(tenant = %tenant.id, campaign = %campaign.id, "newsletter held back: the shop picked no mail server for its marketing");
        return Ok(0);
    };
    let owed = undelivered_subscribers(
        read_pool,
        &tenant.id,
        &campaign.id,
        campaign.language.as_deref(),
        batch,
    )
    .await?;
    let copy = CampaignCopy {
        subject: &campaign.subject,
        html: &campaign.html,
        text: campaign.text.as_deref(),
    };
    let since = campaign.sent_at.unwrap_or(campaign.created_at) as i64 - NEW_PRODUCTS_LOOKBACK_SECS;
    let new_products = new_products(read_pool, cx, tenant, &storefront, since).await?;
    let out = Outgoing {
        newsletter: &newsletter,
        cx,
        sender: &sender,
        tenant,
        keys,
        storefront: &storefront,
        copy: &copy,
        new_products: &new_products,
        claim: Claim::Campaign(&campaign.id),
    };
    let mailed = out.mail(&owed).await?;
    // Finished once nobody is owed any more: fewer than a batch were, and
    // none of them was left for a later sweep (a key not on this node yet).
    if owed.len() < batch as usize && mailed.left_for_later == 0 {
        newsletter.finish_campaign(MAILER, &campaign.id).await?;
        tracing::info!(tenant = %tenant.id, campaign = %campaign.id, "newsletter sent");
    }
    Ok(mailed.sent)
}

/// What one pass over the owed subscribers did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Mailed {
    /// Handed to the mail server.
    pub sent: usize,
    /// Still owed: their key is not on this node yet, so the mail waits for
    /// a later sweep (or another node) rather than being claimed and never
    /// sent. A campaign is not finished while any is left.
    pub left_for_later: usize,
}

/// What a delivery is claimed for.
#[derive(Clone, Copy)]
enum Claim<'a> {
    /// A campaign sent at once.
    Campaign(&'a str),
    /// An edition of a scheduled campaign.
    Edition {
        campaign_id: &'a str,
        edition_id: &'a str,
    },
}

impl Claim<'_> {
    /// The id the deliveries are keyed by.
    fn key(&self) -> &str {
        match self {
            Self::Campaign(id) => id,
            Self::Edition { edition_id, .. } => edition_id,
        }
    }

    fn campaign_id(&self) -> &str {
        match self {
            Self::Campaign(id) => id,
            Self::Edition { campaign_id, .. } => campaign_id,
        }
    }

    fn edition_id(&self) -> Option<&str> {
        match self {
            Self::Campaign(_) => None,
            Self::Edition { edition_id, .. } => Some(edition_id),
        }
    }
}

/// One copy going out to some of a shop's subscribers.
struct Outgoing<'a, E: evento::Executor> {
    newsletter: &'a timada_newsletter::Command<E>,
    cx: &'a Cx,
    sender: &'a ShopSender,
    tenant: &'a timada_tenancy::TenantState,
    keys: &'a dyn Keys,
    storefront: &'a str,
    copy: &'a CampaignCopy<'a>,
    new_products: &'a [NewProductVars],
    claim: Claim<'a>,
}

impl<E: evento::Executor> Outgoing<'_, E> {
    /// Claims and mails each of `rows` still on the list. One who left the
    /// list since the read model was written is recorded as skipped, so
    /// they are not owed again; one whose key is not here yet is left for
    /// a later sweep and counted as such.
    async fn mail(&self, rows: &[SubscriberRow]) -> anyhow::Result<Mailed> {
        let newsletter = self.newsletter;
        let mut mailed = Mailed::default();
        for row in rows {
            // The read model is a moment behind: the write model says whether
            // they are still on the list.
            let gone = match newsletter.load_subscriber(&row.id).await? {
                None => Some("no such subscriber"),
                Some(state) if !state.is_confirmed() => Some("left the list"),
                Some(_) => None,
            };
            if let Some(reason) = gone {
                newsletter
                    .skip_delivery(
                        MAILER,
                        self.claim.campaign_id(),
                        self.claim.edition_id(),
                        &row.id,
                        reason,
                    )
                    .await?;
                continue;
            }
            let Some(state) = newsletter.load_subscriber(&row.id).await? else {
                continue;
            };
            // Opened before the claim: a key not here yet leaves the mail for a
            // later sweep rather than claimed and never sent.
            let (Some(subscriber), Some(token)) = (
                state.open_with(self.keys).await?,
                state.unsubscribe_token(self.keys).await?,
            ) else {
                mailed.left_for_later += 1;
                continue;
            };
            let claimed = match self.claim {
                Claim::Campaign(campaign_id) => {
                    newsletter
                        .claim_delivery(MAILER, campaign_id, &state.id)
                        .await?
                }
                Claim::Edition {
                    campaign_id,
                    edition_id,
                } => {
                    newsletter
                        .claim_edition_delivery(MAILER, campaign_id, edition_id, &state.id)
                        .await?
                }
            };
            if !claimed {
                continue;
            }
            // Claimed, then sent: a failure is recorded, not retried, since a
            // subscriber mailed twice is worse off than one missed.
            let link = unsubscribe_link(self.storefront, &token);
            match send_campaign(
                self.cx,
                self.sender,
                self.tenant,
                &subscriber.email,
                self.copy,
                &link,
                self.new_products,
            )
            .await
            {
                Ok(()) => mailed.sent += 1,
                Err(err) => {
                    tracing::error!(tenant = %self.tenant.id, campaign = %self.claim.key(), subscriber = %state.id, error = %err, "newsletter not delivered");
                    newsletter
                        .fail_delivery(self.claim.key(), &state.id, &err.to_string())
                        .await?;
                }
            }
        }
        Ok(mailed)
    }
}

/// The day `now` (unix seconds) is in UTC.
fn utc_day(now: i64) -> jiff::civil::Date {
    jiff::Timestamp::from_second(now)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(TimeZone::UTC)
        .date()
}

/// Opens the editions of a scheduled campaign whose cycle has begun
/// somewhere on Earth and not ended everywhere, as of `now` (unix
/// seconds); finishes a recurring one whose last cycle is over.
pub async fn open_due_editions<E: evento::Executor + Clone>(
    store: &E,
    tenant: &timada_tenancy::TenantState,
    campaign_id: &str,
    now: i64,
) -> anyhow::Result<()> {
    let newsletter = timada_newsletter::Command::new(store.clone(), &tenant.id);
    let Some(campaign) = newsletter.load_campaign(campaign_id).await? else {
        return Ok(());
    };
    if campaign.status != CampaignStatus::Scheduled {
        return Ok(());
    }
    let Some(schedule) = campaign.schedule.as_ref() else {
        return Ok(());
    };
    if !schedule.is_recurring() {
        // One edition, in the first cycle: rolled on from there while owed.
        if let Some(cycle) = schedule.first_cycle()
            && schedule
                .window_of(cycle)
                .is_some_and(|(opens, _)| opens <= now)
        {
            newsletter
                .open_edition(MAILER, &campaign, &cycle.to_string())
                .await?;
        }
        return Ok(());
    }
    // A cycle may still be open somewhere up to a day after it ended in UTC:
    // look from two days back.
    let from = schedule.cycle_of(
        utc_day(now)
            .checked_sub(jiff::ToSpan::days(2))
            .unwrap_or(utc_day(now)),
    );
    let Some(mut cycle) = schedule.cycle_with_slots(from) else {
        // Past its end date: every edition has had its window.
        newsletter.finish_campaign(MAILER, &campaign.id).await?;
        tracing::info!(tenant = %tenant.id, campaign = %campaign.id, "newsletter schedule over");
        return Ok(());
    };
    while let Some((opens, closes)) = schedule.window_of(cycle) {
        if opens > now {
            break;
        }
        if now < closes {
            newsletter
                .open_edition(MAILER, &campaign, &cycle.to_string())
                .await?;
        }
        match schedule.cycle_with_slots(schedule.next_cycle(cycle)) {
            Some(next) => cycle = next,
            None => break,
        }
    }
    Ok(())
}

/// Mails what an open edition's passed slots owe each time zone, up to
/// `budget` mails, as of `now` (unix seconds); closes (or, one-off and
/// still owing, rolls on) an edition whose window is over. Returns how
/// many were mailed.
#[allow(clippy::too_many_arguments)]
pub async fn send_edition_batch<E: evento::Executor + Clone>(
    store: &E,
    read_pool: &SqlitePool,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    keys: &dyn Keys,
    edition_id: &str,
    budget: u32,
    now: i64,
) -> anyhow::Result<usize> {
    let newsletter = timada_newsletter::Command::new(store.clone(), &tenant.id);
    let Some(edition) = newsletter.load_edition(edition_id).await? else {
        return Ok(0);
    };
    if !edition.is_open() {
        return Ok(0);
    }
    let campaign = newsletter.load_campaign(&edition.campaign_id).await?;
    let (Some(campaign), Some(cycle)) = (campaign, parse_day(&edition.cycle)) else {
        newsletter.close_edition(MAILER, &edition.id).await?;
        return Ok(0);
    };
    let schedule = match (&campaign.schedule, campaign.status) {
        (_, CampaignStatus::Stopped) | (None, _) => {
            newsletter.close_edition(MAILER, &edition.id).await?;
            return Ok(0);
        }
        (Some(schedule), _) => schedule,
    };
    let shop_zone = tenant.shop_time_zone();
    let progress = edition_progress(
        read_pool,
        &tenant.id,
        &edition.id,
        edition.language.as_deref(),
        shop_zone,
    )
    .await?;
    let window = schedule.window_of(cycle);
    if window.is_none_or(|(_, closes)| now >= closes) {
        let owed: i64 = progress.iter().map(|zone| zone.owed).sum();
        if owed > 0
            && !schedule.is_recurring()
            && let Some(next) = schedule.cycle_with_slots(schedule.next_cycle(cycle))
        {
            newsletter
                .roll_edition(MAILER, &edition.id, &next.to_string())
                .await?;
            tracing::info!(tenant = %tenant.id, edition = %edition.id, cycle = %next, "newsletter edition rolled on");
            return Ok(0);
        }
        newsletter.close_edition(MAILER, &edition.id).await?;
        if !schedule.is_recurring() {
            newsletter.finish_campaign(MAILER, &campaign.id).await?;
        }
        tracing::info!(tenant = %tenant.id, edition = %edition.id, "newsletter edition closed");
        return Ok(0);
    }
    let Some(storefront) = timada_orders::Command::new(store.clone(), &tenant.id)
        .storefront_address()
        .await?
    else {
        tracing::warn!(tenant = %tenant.id, edition = %edition.id, "newsletter held back: the shop has no storefront address");
        return Ok(0);
    };
    let Some(sender) = marketing_sender(cx, tenant).await? else {
        tracing::warn!(tenant = %tenant.id, edition = %edition.id, "newsletter held back: the shop picked no mail server for its marketing");
        return Ok(0);
    };
    let copy = CampaignCopy {
        subject: &edition.subject,
        html: &edition.html,
        text: edition.text.as_deref(),
    };
    // New since the edition before this one opened; the first looks back a
    // month.
    let since = timada_newsletter::editions_of_campaign(read_pool, &tenant.id, &campaign.id, 50)
        .await?
        .into_iter()
        .filter(|other| other.id != edition.id && other.opened_at < edition.opened_at as i64)
        .map(|other| other.opened_at)
        .max()
        .unwrap_or(edition.opened_at as i64 - NEW_PRODUCTS_LOOKBACK_SECS);
    let new_products = new_products(read_pool, cx, tenant, &storefront, since).await?;
    let out = Outgoing {
        newsletter: &newsletter,
        cx,
        sender: &sender,
        tenant,
        keys,
        storefront: &storefront,
        copy: &copy,
        new_products: &new_products,
        claim: Claim::Edition {
            campaign_id: &campaign.id,
            edition_id: &edition.id,
        },
    };
    let slots = schedule.slots(cycle);
    let fallback = TimeZone::get(shop_zone).unwrap_or(TimeZone::UTC);
    let mut mailed = 0usize;
    for zone in &progress {
        let left = (budget as usize).saturating_sub(mailed);
        if left == 0 || zone.owed <= 0 {
            continue;
        }
        let tz = TimeZone::get(&zone.zone).unwrap_or_else(|_| fallback.clone());
        // Slots before the edition opened (a schedule started mid-window)
        // are not this edition's.
        let instants: Vec<i64> = slots
            .iter()
            .map(|slot| slot.instant_in(&tz))
            .filter(|at| *at >= edition.opened_at as i64)
            .collect();
        let passed = instants.iter().filter(|at| **at <= now).count();
        if passed == 0 {
            continue;
        }
        let (delivered, owed) = (zone.delivered as u64, zone.owed as u64);
        let due = quota(delivered + owed, passed, instants.len())
            .saturating_sub(delivered)
            .min(owed)
            .min(left as u64);
        if due == 0 {
            continue;
        }
        let rows = undelivered_in_zone(
            read_pool,
            &tenant.id,
            &edition.id,
            edition.language.as_deref(),
            shop_zone,
            &zone.zone,
            due as u32,
        )
        .await?;
        mailed += out.mail(&rows).await?.sent;
    }
    Ok(mailed)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use evento::migrator::Migrate;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx_migrator::Info;
    use timada_core::Key;
    use timada_newsletter::{CampaignDraft, RequestOutcome, RequestSubscription, SOURCE_FORM};
    use topcoat::context::CxTestBuilder;
    use topcoat::mail::{MailConfig, MemoryTransport};

    use super::*;
    use crate::auth::mail::MailSender;

    const SHOP: &str = "https://shop.example";
    const TENANT: &str = "t1";

    #[derive(Default)]
    struct MemoryKeys(Mutex<HashMap<String, Key>>);

    /// The same keys for every tenant.
    struct MemoryRing(Arc<MemoryKeys>);

    impl timada_core::keys::KeyRing for MemoryRing {
        fn for_tenant(&self, _tenant_id: &str) -> Arc<dyn Keys> {
            self.0.clone()
        }
    }

    #[async_trait::async_trait]
    impl Keys for MemoryKeys {
        async fn put(&self, key_id: &str, key: &Key) -> anyhow::Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(key_id.to_owned(), key.clone());
            Ok(())
        }

        async fn get(&self, key_id: &str) -> anyhow::Result<Option<Key>> {
            Ok(self.0.lock().unwrap().get(key_id).cloned())
        }

        async fn delete(&self, key_id: &str) -> anyhow::Result<()> {
            self.0.lock().unwrap().remove(key_id);
            Ok(())
        }
    }

    /// One in-memory database holding both the store and the read model.
    async fn store() -> (evento::Sqlite, SqlitePool) {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let mut migrator = evento::sql_migrator::new::<sqlx::Sqlite>().unwrap();
        migrator
            .add_migrations(timada_newsletter::migrations())
            .unwrap();
        migrator
            .add_migrations(timada_catalog::migrations())
            .unwrap();
        migrator
            .run(&mut *conn, &evento::migrator::Plan::apply_all())
            .await
            .unwrap();
        drop(conn);
        (pool.clone().into(), pool)
    }

    async fn catch_up(store: &evento::Sqlite, pool: &SqlitePool, keys: Arc<MemoryKeys>) {
        timada_catalog::read_model_subscription::<evento::Sqlite>(pool.clone())
            .run_once(store)
            .await
            .unwrap();
        timada_newsletter::read_model_subscription::<evento::Sqlite>(
            pool.clone(),
            Arc::new(MemoryRing(keys)),
        )
        .run_once(store)
        .await
        .unwrap();
    }

    async fn subscribe(store: &evento::Sqlite, keys: &MemoryKeys, email: &str) {
        subscribe_in(store, keys, email, None).await;
    }

    async fn subscribe_in(
        store: &evento::Sqlite,
        keys: &MemoryKeys,
        email: &str,
        time_zone: Option<&str>,
    ) {
        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);
        let RequestOutcome::ToConfirm { token, .. } = newsletter
            .request_subscription(
                keys,
                "storefront",
                RequestSubscription {
                    email: email.to_owned(),
                    language: None,
                    source: SOURCE_FORM.to_owned(),
                    time_zone: time_zone.map(str::to_owned),
                },
                crate::auth::now(),
            )
            .await
            .unwrap()
        else {
            panic!("nothing to mail");
        };
        newsletter
            .confirm_subscription(keys, "storefront", &token, crate::auth::now())
            .await
            .unwrap();
    }

    /// The platform's mailbox and, standing in for `shop`'s two servers,
    /// one for the orders server and one for the news server: a campaign
    /// must only ever reach the last.
    fn mail(
        shop: &timada_tenancy::TenantState,
    ) -> (Cx, MemoryTransport, MemoryTransport, MemoryTransport) {
        let platform = MemoryTransport::new();
        let orders = MemoryTransport::new();
        let news = MemoryTransport::new();
        let shop_mail = crate::shop_mail::ShopMail::new(
            Some(crate::seal::Sealer::new(
                "mail",
                "a-secrets-key-of-thirty-two-chars!!",
            )),
            true,
        );
        shop_mail.stand_in_for(shop, "orders", Arc::new(orders.clone()));
        shop_mail.stand_in_for(shop, "news", Arc::new(news.clone()));
        let cx = CxTestBuilder::new()
            .app_context(MailConfig::builder().transport(platform.clone()).build())
            .app_context(MailSender("Timada <no-reply@example.com>".parse().unwrap()))
            .app_context(shop_mail)
            .app_context(Arc::new(crate::media::Media::new(
                &crate::config::MediaConfig {
                    dir: std::env::temp_dir(),
                    public_url: "https://media.example".to_owned(),
                },
                None,
            )))
            .build();
        (cx, platform, orders, news)
    }

    fn server(id: &str, from_address: &str) -> timada_tenancy::MailServerSettings {
        timada_tenancy::MailServerSettings {
            id: id.into(),
            host: "localhost".into(),
            port: 1025,
            security: timada_tenancy::MAIL_SECURITY_NONE.into(),
            username: None,
            password_sealed: None,
            from_address: from_address.into(),
            from_name: None,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_campaign_goes_out_once_to_each_subscriber_in_batches() {
        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        for email in ["ada@example.com", "grace@example.com", "alan@example.com"] {
            subscribe(&store, &keys, email).await;
        }
        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);
        let campaign = newsletter
            .draft_campaign(
                "user-alice",
                CampaignDraft {
                    subject: "News from {{ shop.name }}".to_owned(),
                    html: r#"<p>New mugs.</p><a href="{{ unsubscribe_url }}">Leave</a>"#.to_owned(),
                    text: None,
                    language: None,
                },
            )
            .await
            .unwrap();
        newsletter
            .send_campaign("user-alice", &campaign)
            .await
            .unwrap();
        catch_up(&store, &pool, keys.clone()).await;
        let mut shop = timada_tenancy::TenantState {
            id: TENANT.into(),
            name: "Acme Tea".into(),
            mail_servers: vec![
                server("orders", "orders@acme.test"),
                server("news", "news@acme.test"),
            ],
            shop_mails_server_id: Some("orders".into()),
            marketing_server_id: Some("news".into()),
            ..Default::default()
        };
        let (cx, platform, orders, transport) = mail(&shop);

        // No storefront yet: no unsubscribe link, nothing goes out.
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            0
        );
        timada_orders::Command::new(store.clone(), TENANT)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();

        // No server picked for marketing: held back, and nobody is claimed,
        // so neither the platform's server nor the one picked for the
        // shop's mails ever carries marketing.
        shop.marketing_server_id = None;
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            0
        );
        assert!(platform.sent().is_empty());
        assert!(orders.sent().is_empty());
        assert!(transport.sent().is_empty());
        shop.marketing_server_id = Some("news".into());

        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            2
        );
        // A sweep before the read model caught up, or another node: the
        // claims are refused, nobody is mailed twice.
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            0
        );
        catch_up(&store, &pool, keys.clone()).await;
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 2)
                .await
                .unwrap(),
            1
        );
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);

        assert!(
            platform.sent().is_empty(),
            "marketing on the platform's server"
        );
        assert!(
            orders.sent().is_empty(),
            "marketing on the shop mails' server"
        );
        let sent = transport.sent();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0].from().unwrap().address(), "news@acme.test");
        let mut to: Vec<String> = sent
            .iter()
            .flat_map(|mail| mail.to().iter().map(|to| to.address().to_owned()))
            .collect();
        to.sort();
        assert_eq!(
            to,
            ["ada@example.com", "alan@example.com", "grace@example.com"]
        );
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        assert!(wire.contains("Subject: News from Acme Tea"));
        assert!(wire.contains("List-Unsubscribe: <https://shop.example/newsletter/unsubscribe/"));
        assert!(wire.contains("List-Unsubscribe-Post: List-Unsubscribe=One-Click"));
    }

    /// A subscriber who left the list after the read model was written is
    /// skipped for good; one whose key is not on this node yet holds the
    /// campaign open until a sweep can mail them.
    #[tokio::test]
    async fn a_campaign_waits_for_a_missing_key_and_skips_who_left() {
        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        for email in ["ada@example.com", "grace@example.com", "alan@example.com"] {
            subscribe(&store, &keys, email).await;
        }
        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);
        let campaign = newsletter
            .draft_campaign(
                "user-alice",
                CampaignDraft {
                    subject: "News".to_owned(),
                    html: r#"<p>New mugs.</p><a href="{{ unsubscribe_url }}">Leave</a>"#.to_owned(),
                    text: None,
                    language: None,
                },
            )
            .await
            .unwrap();
        newsletter
            .send_campaign("user-alice", &campaign)
            .await
            .unwrap();
        timada_orders::Command::new(store.clone(), TENANT)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
        catch_up(&store, &pool, keys.clone()).await;
        let shop = shop();
        let (cx, _platform, _orders, transport) = mail(&shop);

        // The read model owes all three. Ada leaves (the read model does not
        // know yet); Grace's key is not on this node.
        let owed = undelivered_subscribers(&pool, TENANT, &campaign, None, 10)
            .await
            .unwrap();
        assert_eq!(owed.len(), 3);
        let by_email = |email: &str| {
            owed.iter()
                .find(|row| row.email == email)
                .unwrap()
                .id
                .clone()
        };
        let ada = by_email("ada@example.com");
        let grace = newsletter
            .load_subscriber(&by_email("grace@example.com"))
            .await
            .unwrap()
            .unwrap();
        newsletter.unsubscribe("storefront", &ada).await.unwrap();
        let grace_key = keys.get(&grace.key_id).await.unwrap().unwrap();
        keys.delete(&grace.key_id).await.unwrap();

        // Alan is mailed, Ada is skipped for good, Grace waits: not finished.
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 10)
                .await
                .unwrap(),
            1
        );
        assert_eq!(recipients(&transport), ["alan@example.com"]);
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sending, "Grace is still owed");
        catch_up(&store, &pool, keys.clone()).await;
        let owed = undelivered_subscribers(&pool, TENANT, &campaign, None, 10)
            .await
            .unwrap();
        assert_eq!(owed.len(), 1, "Ada is not owed any more: {owed:?}");
        assert_eq!(owed[0].email, "grace@example.com");

        // The key arrives: Grace is mailed and the campaign is finished.
        keys.put(&grace.key_id, &grace_key).await.unwrap();
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 10)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            recipients(&transport),
            ["alan@example.com", "grace@example.com"]
        );
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);
        catch_up(&store, &pool, keys.clone()).await;
        let row = timada_newsletter::campaign_row(&pool, TENANT, &campaign)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((row.delivered, row.failed, row.skipped), (2, 0, 1));
    }

    /// Unix seconds of a UTC moment.
    fn at(moment: &str) -> i64 {
        moment.parse::<jiff::Timestamp>().unwrap().as_second()
    }

    fn day(value: &str) -> jiff::civil::Date {
        value.parse().unwrap()
    }

    async fn scheduled_campaign(
        store: &evento::Sqlite,
        schedule: timada_newsletter::schedule::Schedule,
    ) -> String {
        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);
        let campaign = newsletter
            .draft_campaign(
                "user-alice",
                CampaignDraft {
                    subject: "News".to_owned(),
                    html: r#"<a href="{{ unsubscribe_url }}">Leave</a>"#.to_owned(),
                    text: None,
                    language: None,
                },
            )
            .await
            .unwrap();
        let today = day("2026-10-29");
        newsletter
            .set_schedule("user-alice", &campaign, schedule, today)
            .await
            .unwrap();
        newsletter
            .activate_schedule("user-alice", &campaign, today)
            .await
            .unwrap();
        campaign
    }

    fn shop() -> timada_tenancy::TenantState {
        timada_tenancy::TenantState {
            id: TENANT.into(),
            name: "Acme Tea".into(),
            mail_servers: vec![
                server("orders", "orders@acme.test"),
                server("news", "news@acme.test"),
            ],
            shop_mails_server_id: Some("orders".into()),
            marketing_server_id: Some("news".into()),
            ..Default::default()
        }
    }

    /// Who got a mail, sorted.
    fn recipients(transport: &MemoryTransport) -> Vec<String> {
        let mut to: Vec<String> = transport
            .sent()
            .iter()
            .flat_map(|mail| mail.to().iter().map(|to| to.address().to_owned()))
            .collect();
        to.sort();
        to
    }

    #[tokio::test]
    async fn a_scheduled_edition_reaches_each_zone_at_its_own_nine_oclock() {
        use timada_newsletter::schedule::{Cadence, Schedule};

        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        for n in 0..4 {
            subscribe_in(
                &store,
                &keys,
                &format!("paris{n}@example.com"),
                Some("Europe/Paris"),
            )
            .await;
        }
        for n in 0..2 {
            subscribe_in(
                &store,
                &keys,
                &format!("ny{n}@example.com"),
                Some("America/New_York"),
            )
            .await;
        }
        // No zone: the shop's, UTC.
        subscribe(&store, &keys, "nowhere@example.com").await;
        timada_orders::Command::new(store.clone(), TENANT)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
        // Once, on Tuesday 8 January 2030, 9:00 to 10:00, every 30 minutes:
        // two slots.
        let campaign = scheduled_campaign(
            &store,
            Schedule {
                cadence: Cadence::Once,
                weekdays: vec![],
                month_day: None,
                starts_on: day("2030-01-08"),
                ends_on: None,
                window_start: 9 * 60,
                window_end: Some(10 * 60),
                pace_minutes: 30,
            },
        )
        .await;
        let shop = shop();
        let (cx, _, _, transport) = mail(&shop);
        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);

        // Too early anywhere: nothing opens.
        open_due_editions(&store, &shop, &campaign, at("2030-01-07T18:00:00Z"))
            .await
            .unwrap();
        catch_up(&store, &pool, keys.clone()).await;
        assert!(open_editions(&pool).await.unwrap().is_empty());

        // 9:00 in Kiritimati: open, but nobody's slot has come.
        open_due_editions(&store, &shop, &campaign, at("2030-01-07T19:00:00Z"))
            .await
            .unwrap();
        catch_up(&store, &pool, keys.clone()).await;
        let [(_, edition)] = open_editions(&pool).await.unwrap().try_into().unwrap();
        let sweep = async |moment: &str| {
            let mailed =
                send_edition_batch(&store, &pool, &cx, &shop, &*keys, &edition, 100, at(moment))
                    .await
                    .unwrap();
            catch_up(&store, &pool, keys.clone()).await;
            mailed
        };
        assert_eq!(sweep("2030-01-07T19:00:00Z").await, 0);
        assert_eq!(sweep("2030-01-08T07:59:00Z").await, 0);

        // 9:00 in Paris (UTC+1): half of Paris, nobody else.
        assert_eq!(sweep("2030-01-08T08:00:00Z").await, 2);
        // Looked at again before the next slot: nothing more.
        assert_eq!(sweep("2030-01-08T08:10:00Z").await, 0);
        // 9:30 in Paris: the rest of Paris.
        assert_eq!(sweep("2030-01-08T08:30:00Z").await, 2);
        // 9:00 in UTC, the shop's zone: its one subscriber.
        assert_eq!(sweep("2030-01-08T09:00:00Z").await, 1);
        // 9:00 and 9:30 in New York (UTC-5).
        assert_eq!(sweep("2030-01-08T13:59:00Z").await, 0);
        assert_eq!(sweep("2030-01-08T14:00:00Z").await, 1);
        assert_eq!(sweep("2030-01-08T14:30:00Z").await, 1);
        assert_eq!(transport.sent().len(), 7);

        // Its window over everywhere: closed, and the campaign done.
        assert_eq!(sweep("2030-01-08T22:00:00Z").await, 0);
        let state = newsletter.load_edition(&edition).await.unwrap().unwrap();
        assert!(!state.is_open());
        let campaign = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(campaign.status, CampaignStatus::Sent);
        assert_eq!(recipients(&transport).len(), 7);
    }

    #[tokio::test]
    async fn a_recurring_campaign_mails_each_subscriber_once_per_edition() {
        use timada_newsletter::schedule::{Cadence, Schedule};

        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        subscribe_in(&store, &keys, "ada@example.com", Some("Europe/Paris")).await;
        timada_orders::Command::new(store.clone(), TENANT)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
        // Every day at 9:00, for two days.
        let campaign = scheduled_campaign(
            &store,
            Schedule {
                cadence: Cadence::Daily,
                weekdays: vec![],
                month_day: None,
                starts_on: day("2030-01-08"),
                ends_on: Some(day("2030-01-09")),
                window_start: 9 * 60,
                window_end: None,
                pace_minutes: 30,
            },
        )
        .await;
        let shop = shop();
        let (cx, _, _, transport) = mail(&shop);
        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);

        for (moment, editions) in [("2030-01-08T08:00:00Z", 1), ("2030-01-09T08:00:00Z", 2)] {
            open_due_editions(&store, &shop, &campaign, at(moment))
                .await
                .unwrap();
            catch_up(&store, &pool, keys.clone()).await;
            for (_, edition) in open_editions(&pool).await.unwrap() {
                send_edition_batch(&store, &pool, &cx, &shop, &*keys, &edition, 100, at(moment))
                    .await
                    .unwrap();
                catch_up(&store, &pool, keys.clone()).await;
                // Twice in the same slot: no second mail.
                send_edition_batch(&store, &pool, &cx, &shop, &*keys, &edition, 100, at(moment))
                    .await
                    .unwrap();
                catch_up(&store, &pool, keys.clone()).await;
            }
            assert_eq!(transport.sent().len(), editions);
        }
        assert_eq!(
            timada_newsletter::editions_of_campaign(&pool, TENANT, &campaign, 10)
                .await
                .unwrap()
                .len(),
            2
        );

        // Past its end date, the windows closed: the campaign is done.
        let after = at("2030-01-12T00:00:00Z");
        for (_, edition) in open_editions(&pool).await.unwrap() {
            send_edition_batch(&store, &pool, &cx, &shop, &*keys, &edition, 100, after)
                .await
                .unwrap();
        }
        open_due_editions(&store, &shop, &campaign, after)
            .await
            .unwrap();
        catch_up(&store, &pool, keys.clone()).await;
        assert!(open_editions(&pool).await.unwrap().is_empty());
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);
    }

    #[tokio::test]
    async fn a_campaign_shows_the_products_just_in() {
        let (store, pool) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        subscribe(&store, &keys, "ada@example.com").await;
        timada_orders::Command::new(store.clone(), TENANT)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
        let catalog = timada_catalog::Command::new(store.clone(), TENANT);
        let mug = catalog
            .create_product(
                "user-alice",
                timada_catalog::CreateProduct {
                    name: "Blue mug".to_owned(),
                    slug: Some("blue-mug".to_owned()),
                },
            )
            .await
            .unwrap();
        catalog
            .price_product("user-alice", &mug.id, 1250, "EUR")
            .await
            .unwrap();
        catalog
            .publish_product("user-alice", &mug.id)
            .await
            .unwrap();
        // A draft is not on sale: not shown.
        catalog
            .create_product(
                "user-alice",
                timada_catalog::CreateProduct {
                    name: "Secret teapot".to_owned(),
                    slug: None,
                },
            )
            .await
            .unwrap();

        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);
        let campaign = newsletter
            .draft_campaign(
                "user-alice",
                CampaignDraft {
                    subject: "Just in".to_owned(),
                    html: r#"{% for product in new_products %}<a href="{{ product.url }}">{{ product.name }}</a> {{ product.price }}{% endfor %}<a href="{{ unsubscribe_url }}">Leave</a>"#.to_owned(),
                    text: None,
                    language: None,
                },
            )
            .await
            .unwrap();
        newsletter
            .send_campaign("user-alice", &campaign)
            .await
            .unwrap();
        catch_up(&store, &pool, keys.clone()).await;
        let shop = shop();
        let (cx, _, _, transport) = mail(&shop);
        assert_eq!(
            send_batch(&store, &pool, &cx, &shop, &*keys, &campaign, 10)
                .await
                .unwrap(),
            1
        );
        let sent = transport.sent();
        let wire = String::from_utf8(sent[0].clone().formatted(&cx).unwrap()).unwrap();
        let wire = wire.replace("&#x2f;", "/").replace("=\r\n", "");
        assert!(wire.contains("Blue mug"), "{wire}");
        assert!(wire.contains("https://shop.example/p/blue-mug"), "{wire}");
        assert!(!wire.contains("Secret teapot"));
    }
}
