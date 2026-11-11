//! Sends the shops' newsletter campaigns: once a merchant sends one, every
//! node mails its own part of the confirmed subscribers
//! ([`Share`](timada_core::Share), among the nodes up), on a timer, from the
//! shop. Each subscriber's mail is claimed first (`Delivery`), so a
//! subscriber gets a campaign once even while two nodes disagree on who is
//! up; a campaign with nobody left to mail is finished.
//!
//! Each look starts a lane per shop with mails going out, a bounded number
//! of shops at once (those served longest ago first), each lane on its own:
//! it mails one subscriber after the other through the shop's server until
//! shortly before the next look (which then finds it over and starts it
//! again) or the most a look sends per campaign went out, whichever comes
//! first, so a fast server sends many and a slow one few, and a slow server
//! holds its own shop and nothing else. The first send the server fails
//! (rather than the address) ends the lane for the look: its other
//! subscribers wait for the next look rather than each be claimed and
//! failed. A node walks each list by subscriber id and goes on from the last
//! one it looked at ([`Walk`]) rather than through everyone mailed already;
//! once its own part is done, it walks everyone's, so a node that is up but
//! not sending holds nobody back for good.
//!
//! A scheduled campaign goes out an edition per cycle of its schedule
//! ([`timada_newsletter::schedule`]): an edition opens when its first slot
//! comes somewhere on Earth, and at each look every time zone's subscribers
//! get the share their passed slots owe them, so 9:00 is 9:00 where each
//! subscriber lives (the shop's zone for those whose zone is unknown). The
//! shares come from the read model's counts; at a zone's last slot whoever
//! is still owed there goes. The look's interval bounds how close to a slot
//! its mails go. Once its window has closed everywhere, an edition is
//! closed; a one-off one still owing mails rolls on to the schedule's next
//! cycle instead.
//!
//! A campaign goes through the mail server the shop picked for its
//! marketing and nothing else: a shop that picked none (or deleted it
//! mid-campaign) has its campaign held back, claimed by nobody, until it
//! picks one again. A server switched mid-campaign sends the next mails.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;

use crate::db::Db;
use timada_core::keys::KeyRing;
use timada_core::{Keys, Share};
use timada_newsletter::schedule::{parse_day, quota};
use timada_newsletter::{
    CampaignStatus, SubscriberRow, anyone_owed, edition_progress, open_editions,
    scheduled_campaigns, sending_campaigns, undelivered_in_zone, undelivered_subscribers,
};
use timada_tenancy::NewProductVars;
use tokio::task::JoinHandle;
use topcoat::context::{AppContext, Cx};

use crate::cluster::Executor;
use crate::mail::{CampaignCopy, marketing_sender, send_campaign};
use crate::mail_fault::Attempt;
use crate::newsletter::unsubscribe_link;
use crate::shop_mail::ShopSender;

/// Who the deliveries say mailed them.
pub const MAILER: &str = "timada-dashboard";

/// The most products a campaign shows as just in.
const NEW_PRODUCTS_SHOWN: i64 = 12;

/// How far back a campaign without an earlier edition looks for new
/// products, in seconds.
const NEW_PRODUCTS_LOOKBACK_SECS: i64 = 30 * 24 * 3600;

/// How many shops send at once from one node. Within a shop, one mail
/// after the other.
const LANES: usize = 16;

/// How many owed subscribers one read of the read model takes.
const PAGE: u32 = 100;

/// The shop's published products made after `since` (unix seconds), as a
/// campaign shows them: linked to their page on the storefront
/// (`{storefront}/p/{slug}`), with their first picture.
pub async fn new_products(
    db: &Db,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    storefront: &str,
    since: i64,
) -> anyhow::Result<Vec<NewProductVars>> {
    let media = topcoat::context::app_context::<Arc<crate::media::Media>>(cx);
    Ok(
        timada_catalog::new_products(db, &tenant.id, since, NEW_PRODUCTS_SHOWN)
            .await?
            .into_iter()
            .map(|product| NewProductVars {
                url: format!("{}/p/{}", storefront.trim_end_matches('/'), product.slug),
                price: product.price_label(),
                // The row holds the listing rendition's path below the
                // tenant's picture address.
                image_url: product
                    .thumbnail
                    .as_deref()
                    .map(|file| format!("{}/{file}", media.tenant_url(&tenant.id))),
                name: product.name,
            })
            .collect(),
    )
}

/// This node's place among the nodes up, as of now: which subscribers it
/// mails.
pub type Place = Arc<dyn Fn() -> Share + Send + Sync>;

/// The sweep, with what it needs outside any request.
pub struct NewsletterSends {
    /// The read models: the campaigns, the editions, the subscribers, the
    /// catalog a campaign shows.
    db: Db,
    /// The store, every tenant's.
    store: Executor,
    mail: Arc<AppContext>,
    /// Every tenant's keys, which open the subscribers' addresses.
    keys: Arc<dyn KeyRing>,
    place: Place,
    /// The most mails one campaign (or edition) sends per look from this
    /// node.
    per_look: u32,
    /// How often the campaigns are looked at, and so how long a shop's lane
    /// sends per look.
    every: Duration,
    /// The shops whose lane runs: not started again until it ends, and no
    /// more than [`LANES`] at once.
    busy: Mutex<HashSet<String>>,
    /// When each shop's lane last started on this node: with more shops
    /// sending than [`LANES`], those served longest ago go first.
    served: Mutex<HashMap<String, Instant>>,
    /// Where this node's walk over each campaign's (or edition's) list
    /// stands, by its id.
    walks: Mutex<HashMap<String, Walk>>,
}

/// A shop's lane while it runs: the shop leaves the busy set when the lane
/// ends, however it ends.
struct Running<'a> {
    busy: &'a Mutex<HashSet<String>>,
    tenant_id: &'a str,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.busy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(self.tenant_id);
    }
}

/// What a shop has going out.
enum Item {
    /// A campaign sent at once.
    Campaign(String),
    /// An open edition of a scheduled campaign.
    Edition(String),
}

impl Item {
    fn id(&self) -> &str {
        match self {
            Self::Campaign(id) | Self::Edition(id) => id,
        }
    }
}

impl NewsletterSends {
    pub fn new(
        db: Db,
        store: Executor,
        mail: Arc<AppContext>,
        keys: Arc<dyn KeyRing>,
        place: Place,
        per_look: u32,
        every: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            mail,
            keys,
            place,
            per_look: per_look.max(1),
            every,
            busy: Mutex::default(),
            served: Mutex::default(),
            walks: Mutex::default(),
        })
    }

    /// Looks every `every`, the first time right away. The lanes run on
    /// their own: a shop whose server is slow holds its lane, never the next
    /// look.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(self.every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(err) = self.look(crate::auth::now() as i64).await {
                    tracing::error!(error = %format!("{err:#}"), "newsletter campaigns not swept");
                }
            }
        })
    }

    /// Looks at `now` (unix seconds) and waits for every lane; returns how
    /// many mails went out. What [`start`](Self::start) does on each beat,
    /// for a test.
    #[cfg(test)]
    pub(crate) async fn sweep(self: &Arc<Self>, now: i64) -> anyhow::Result<usize> {
        let mut sent = 0;
        for lane in self.look(now).await? {
            sent += lane.await.unwrap_or(0);
        }
        Ok(sent)
    }

    /// Opens the editions due of the scheduled campaigns (the same write
    /// from any node), then starts a lane for each shop with mails going out
    /// and no lane running, those served longest ago first, up to [`LANES`]
    /// running at once. A campaign that fails is logged and the others
    /// still go; a read model that cannot be read ends the look, logged by
    /// the caller.
    async fn look(self: &Arc<Self>, now: i64) -> anyhow::Result<Vec<JoinHandle<usize>>> {
        let started = Instant::now();
        let share = (self.place)();
        for (tenant_id, campaign_id) in scheduled_campaigns(&self.db).await? {
            let tenant = match self.active_tenant(&tenant_id).await {
                Ok(Some(tenant)) => tenant,
                Ok(None) => continue,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, campaign = %campaign_id, error = %format!("{err:#}"), "newsletter editions not opened: the store did not answer");
                    continue;
                }
            };
            if let Err(err) = open_due_editions(&self.store, &tenant, &campaign_id, now).await {
                tracing::error!(tenant = %tenant_id, campaign = %campaign_id, error = %err, "newsletter editions not opened");
            }
        }
        // The editions first in their shop's lane: their slots name an hour.
        let mut going_out: BTreeMap<String, Vec<Item>> = BTreeMap::new();
        for (tenant_id, edition_id) in open_editions(&self.db).await? {
            going_out
                .entry(tenant_id)
                .or_default()
                .push(Item::Edition(edition_id));
        }
        for (tenant_id, campaign_id) in sending_campaigns(&self.db).await? {
            going_out
                .entry(tenant_id)
                .or_default()
                .push(Item::Campaign(campaign_id));
        }
        let ids: HashSet<&str> = going_out.values().flatten().map(Item::id).collect();
        self.walks
            .lock()
            .unwrap()
            .retain(|id, _| ids.contains(id.as_str()));
        self.served
            .lock()
            .unwrap()
            .retain(|tenant_id, _| going_out.contains_key(tenant_id));
        let mut going_out: Vec<(String, Vec<Item>)> = going_out.into_iter().collect();
        {
            let served = self.served.lock().unwrap();
            going_out.sort_by_key(|(tenant_id, _)| served.get(tenant_id).copied());
        }
        let mut lanes = Vec::new();
        for (tenant_id, items) in going_out {
            {
                let mut busy = self.busy.lock().unwrap();
                if busy.len() >= LANES || !busy.insert(tenant_id.clone()) {
                    continue;
                }
            }
            self.served
                .lock()
                .unwrap()
                .insert(tenant_id.clone(), started);
            let sends = self.clone();
            lanes.push(tokio::spawn(async move {
                let _running = Running {
                    busy: &sends.busy,
                    tenant_id: &tenant_id,
                };
                sends.lane(&tenant_id, &items, share, started, now).await
            }));
        }
        Ok(lanes)
    }

    /// One shop's campaigns and editions, one after the other, each with an
    /// even part of the time until shortly before the next look (what one
    /// leaves, the next may use), counted from when the look `started`; how
    /// many mails went out. A send the shop's server failed ends the lane:
    /// its other campaigns go through the same server.
    async fn lane(
        &self,
        tenant_id: &str,
        items: &[Item],
        share: Share,
        started: Instant,
        now: i64,
    ) -> usize {
        // Over a tenth of the interval before the next look, which then
        // finds the shop free and starts its lane again.
        let window = self.every - self.every / 10;
        let tenant = match self.active_tenant(tenant_id).await {
            Ok(Some(tenant)) => tenant,
            Ok(None) => return 0,
            Err(err) => {
                tracing::error!(tenant = %tenant_id, error = %format!("{err:#}"), "newsletter mails not sent: the store did not answer");
                return 0;
            }
        };
        let cx = Cx::new(self.mail.clone());
        let keys = self.keys.for_tenant(tenant_id);
        let mut sent = 0;
        for (n, item) in items.iter().enumerate() {
            let until = started + window * (n as u32 + 1) / items.len() as u32;
            let walk = self
                .walks
                .lock()
                .unwrap()
                .remove(item.id())
                .filter(|walk| walk.share == share)
                .unwrap_or_else(|| Walk::new(share));
            let mut pass = Pass::new(walk, self.per_look, until);
            let outcome = match item {
                Item::Campaign(id) => {
                    send_batch(&self.store, &self.db, &cx, &tenant, &*keys, id, &mut pass).await
                }
                Item::Edition(id) => {
                    send_edition_batch(
                        &self.store,
                        &self.db,
                        &cx,
                        &tenant,
                        &*keys,
                        id,
                        &mut pass,
                        now,
                    )
                    .await
                }
            };
            let halted = pass.halted;
            self.walks
                .lock()
                .unwrap()
                .insert(item.id().to_owned(), pass.walk);
            match (outcome, item) {
                (Ok(mailed), _) => sent += mailed,
                (Err(err), Item::Campaign(id)) => {
                    tracing::error!(tenant = %tenant_id, campaign = %id, error = %format!("{err:#}"), "newsletter batch not sent")
                }
                (Err(err), Item::Edition(id)) => {
                    tracing::error!(tenant = %tenant_id, edition = %id, error = %format!("{err:#}"), "newsletter edition batch not sent")
                }
            }
            if halted {
                break;
            }
        }
        sent
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
}

/// Where a node's walk over one list of owed subscribers stands, in each
/// time zone (`""` for a campaign sent at once), and this node's part of
/// the subscribers (another part starts every list again).
///
/// A walk is one pass after another over the list, each from the top: a
/// look goes on from the last subscriber the one before looked at, and a
/// pass that reached the end leaves the next one anyone left for later or
/// added since. A node walks its own part until a whole pass over it found
/// nobody to mail while someone is still owed, then everyone's: a node that
/// is up but not sending (copying its store, catching up, refused writes)
/// holds its part back for a pass or two, not for good, and the claims keep
/// two nodes from mailing anyone twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    share: Share,
    zones: HashMap<String, ZonePass>,
}

/// Where the pass over one time zone's list stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ZonePass {
    /// The last subscriber looked at; `""` from the top.
    after: String,
    /// Whether this pass claimed anyone.
    claimed: bool,
    /// Walking everyone's part, this node's being done.
    helping: bool,
}

impl Walk {
    pub fn new(share: Share) -> Self {
        Self {
            share,
            zones: HashMap::new(),
        }
    }

    /// The part the pass over `zone` reads: this node's, or everyone's.
    fn part(&self, zone: &str) -> Share {
        match self.zones.get(zone) {
            Some(pass) if pass.helping => Share::ALONE,
            _ => self.share,
        }
    }

    /// The last subscriber looked at in `zone`; `""` from the top.
    fn after(&self, zone: &str) -> &str {
        self.zones.get(zone).map_or("", |pass| pass.after.as_str())
    }

    fn zone(&mut self, zone: &str) -> &mut ZonePass {
        self.zones.entry(zone.to_owned()).or_default()
    }
}

/// What one look lets a node send of one campaign (or edition).
pub struct Pass {
    /// Where this node's walk over the list stands.
    walk: Walk,
    /// The most mails it hands over (or tries).
    limit: u32,
    /// When it stops: the look's interval, or its part of it.
    until: Instant,
    /// Handed over (or tried) so far.
    mailed: u32,
    /// The shop's server failed a send: nothing more goes out this look.
    halted: bool,
}

impl Pass {
    pub fn new(walk: Walk, limit: u32, until: Instant) -> Self {
        Self {
            walk,
            limit,
            until,
            mailed: 0,
            halted: false,
        }
    }

    /// Whether nothing more goes out this look.
    fn spent(&self) -> bool {
        self.halted || self.mailed >= self.limit || Instant::now() >= self.until
    }

    /// How many more it may hand over.
    fn room(&self) -> u32 {
        self.limit.saturating_sub(self.mailed)
    }
}

/// Mails the campaign to the subscribers this node's part still owes, as
/// far as `pass` lets it, then finishes the campaign once its walk reached
/// the end and nobody is owed any more, whichever node's part they are.
/// Returns how many were mailed.
pub async fn send_batch<E: evento::Executor + Clone>(
    store: &E,
    db: &Db,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    keys: &dyn Keys,
    campaign_id: &str,
    pass: &mut Pass,
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
    let copy = CampaignCopy {
        subject: &campaign.subject,
        html: &campaign.html,
        text: campaign.text.as_deref(),
    };
    let since = campaign.sent_at.unwrap_or(campaign.created_at) as i64 - NEW_PRODUCTS_LOOKBACK_SECS;
    let new_products = new_products(db, cx, tenant, &storefront, since).await?;
    let out = Outgoing {
        newsletter: &newsletter,
        db,
        cx,
        sender: &sender,
        tenant,
        keys,
        storefront: &storefront,
        copy: &copy,
        new_products: &new_products,
        language: campaign.language.as_deref(),
        claim: Claim::Campaign(&campaign.id),
    };
    let (sent, owed) = out.walk(pass, List::All, u32::MAX).await?;
    // Finished once the walk reached the end and nobody is owed any more,
    // this node's part or another's: one left for later (a key not here yet)
    // is still owed.
    if owed == Some(false) {
        newsletter.finish_campaign(MAILER, &campaign.id).await?;
        tracing::info!(tenant = %tenant.id, campaign = %campaign.id, "newsletter sent");
    }
    Ok(sent)
}

/// What one pass over some owed subscribers did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Mailed {
    /// Handed to the mail server.
    sent: usize,
    /// Claimed: handed over, or tried and failed.
    claimed: usize,
    /// How many of the rows given were looked at before the pass ran out.
    looked_at: usize,
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

/// Which of the owed subscribers a walk reads.
#[derive(Clone, Copy)]
enum List<'a> {
    /// Everyone owed.
    All,
    /// Those living in `zone`; `shop_zone` stands in for those whose zone
    /// is unknown.
    Zone { shop_zone: &'a str, zone: &'a str },
}

impl List<'_> {
    /// What the walk of the list is kept under.
    fn zone(&self) -> &str {
        match self {
            Self::All => "",
            Self::Zone { zone, .. } => zone,
        }
    }
}

/// One copy going out to some of a shop's subscribers.
struct Outgoing<'a, E: evento::Executor> {
    newsletter: &'a timada_newsletter::Command<E>,
    db: &'a Db,
    cx: &'a Cx,
    sender: &'a ShopSender,
    tenant: &'a timada_tenancy::TenantState,
    keys: &'a dyn Keys,
    storefront: &'a str,
    copy: &'a CampaignCopy<'a>,
    new_products: &'a [NewProductVars],
    /// Only the subscribers in this language get it; none is everyone.
    language: Option<&'a str>,
    claim: Claim<'a>,
}

impl<E: evento::Executor> Outgoing<'_, E> {
    /// Up to `limit` subscribers of `list` this node's part still owes, from
    /// the first after `after`.
    async fn owed(
        &self,
        list: List<'_>,
        share: Share,
        after: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<SubscriberRow>> {
        let tenant_id = &self.tenant.id;
        match list {
            List::All => {
                undelivered_subscribers(
                    self.db,
                    tenant_id,
                    self.claim.key(),
                    self.language,
                    share,
                    after,
                    limit,
                )
                .await
            }
            List::Zone { shop_zone, zone } => {
                undelivered_in_zone(
                    self.db,
                    tenant_id,
                    self.claim.key(),
                    self.language,
                    shop_zone,
                    zone,
                    share,
                    after,
                    limit,
                )
                .await
            }
        }
    }

    /// Mails up to `due` of `list`'s owed subscribers, a page at a time from
    /// where this node's walk of it stopped, as far as `pass` lets it.
    /// Returns how many were mailed and, once the pass reached the end of
    /// the list (the next one starts from the top), whether anyone is still
    /// owed there, whichever node's part.
    async fn walk(
        &self,
        pass: &mut Pass,
        list: List<'_>,
        due: u32,
    ) -> anyhow::Result<(usize, Option<bool>)> {
        let zone = list.zone();
        let mut sent = 0;
        let mut due = due;
        loop {
            if due == 0 || pass.spent() {
                return Ok((sent, None));
            }
            let limit = due.min(pass.room()).min(PAGE);
            let rows = self
                .owed(list, pass.walk.part(zone), pass.walk.after(zone), limit)
                .await?;
            let mailed = self.mail(&rows, pass, zone).await?;
            sent += mailed.sent;
            due = due.saturating_sub(mailed.claimed as u32);
            if mailed.looked_at < rows.len() || rows.len() == limit as usize {
                continue;
            }
            let owed = !self.owed(list, Share::ALONE, "", 1).await?.is_empty();
            let zone_pass = pass.walk.zone(zone);
            // A whole pass found nobody of this node's part to mail, yet
            // someone is owed: whoever holds their part does not send.
            if !zone_pass.claimed && owed {
                zone_pass.helping = true;
            }
            zone_pass.after.clear();
            zone_pass.claimed = false;
            return Ok((sent, Some(owed)));
        }
    }

    /// Claims and mails each of `rows` still on the list, as far as `pass`
    /// lets it, moving the walk of `zone` on past each. One who left the
    /// list since the read model was written is recorded as skipped, so
    /// they are not owed again; one whose key is not here yet is left for
    /// the walk's next time through. A send the server failed (rather than
    /// the address) is recorded and ends the pass: the next subscriber
    /// would fail the same way.
    async fn mail(
        &self,
        rows: &[SubscriberRow],
        pass: &mut Pass,
        zone: &str,
    ) -> anyhow::Result<Mailed> {
        let newsletter = self.newsletter;
        let mut mailed = Mailed::default();
        for row in rows {
            if pass.spent() {
                break;
            }
            mailed.looked_at += 1;
            pass.walk.zone(zone).after.clone_from(&row.id);
            // The read model is a moment behind: the write model says whether
            // they are still on the list.
            let state = match newsletter.load_subscriber(&row.id).await? {
                Some(state) if state.is_confirmed() => state,
                gone => {
                    let reason = if gone.is_some() {
                        "left the list"
                    } else {
                        "no such subscriber"
                    };
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
            };
            // Opened before the claim: a key not here yet leaves the mail for a
            // later look rather than claimed and never sent.
            let (Some(subscriber), Some(token)) = (
                state.open_with(self.keys).await?,
                state.unsubscribe_token(self.keys).await?,
            ) else {
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
            mailed.claimed += 1;
            pass.mailed += 1;
            pass.walk.zone(zone).claimed = true;
            // Claimed, then sent: a failure is recorded, not retried, since a
            // subscriber mailed twice is worse off than one missed.
            let link = unsubscribe_link(self.storefront, &token);
            let reason = match crate::mail_fault::attempt(send_campaign(
                self.cx,
                self.sender,
                self.tenant,
                &subscriber.email,
                self.copy,
                &link,
                self.new_products,
            ))
            .await
            {
                Attempt::Sent => {
                    mailed.sent += 1;
                    continue;
                }
                Attempt::Undeliverable(reason) => {
                    tracing::warn!(tenant = %self.tenant.id, campaign = %self.claim.key(), subscriber = %state.id, reason = %reason, "newsletter not delivered: the address was refused");
                    reason
                }
                Attempt::NotSent(reason) => {
                    tracing::error!(tenant = %self.tenant.id, campaign = %self.claim.key(), subscriber = %state.id, reason = %reason, "newsletter not delivered: the shop's mail server failed; its other subscribers wait for the next look");
                    pass.halted = true;
                    reason
                }
            };
            newsletter
                .fail_delivery(self.claim.key(), &state.id, &reason)
                .await?;
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

/// Mails what an open edition's passed slots owe each time zone, this
/// node's part of it, as far as `pass` lets it, as of `now` (unix
/// seconds); closes (or, one-off and still owing, rolls on) an edition
/// whose window is over. Returns how many were mailed.
#[allow(clippy::too_many_arguments)]
pub async fn send_edition_batch<E: evento::Executor + Clone>(
    store: &E,
    db: &Db,
    cx: &Cx,
    tenant: &timada_tenancy::TenantState,
    keys: &dyn Keys,
    edition_id: &str,
    pass: &mut Pass,
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
    let window = schedule.window_of(cycle);
    if window.is_none_or(|(_, closes)| now >= closes) {
        if !schedule.is_recurring()
            && let Some(next) = schedule.cycle_with_slots(schedule.next_cycle(cycle))
            && anyone_owed(db, &tenant.id, &edition.id, edition.language.as_deref()).await?
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
    let since = timada_newsletter::editions_of_campaign(db, &tenant.id, &campaign.id, 50)
        .await?
        .into_iter()
        .filter(|other| other.id != edition.id && other.opened_at < edition.opened_at as i64)
        .map(|other| other.opened_at)
        .max()
        .unwrap_or(edition.opened_at as i64 - NEW_PRODUCTS_LOOKBACK_SECS);
    let new_products = new_products(db, cx, tenant, &storefront, since).await?;
    let out = Outgoing {
        newsletter: &newsletter,
        db,
        cx,
        sender: &sender,
        tenant,
        keys,
        storefront: &storefront,
        copy: &copy,
        new_products: &new_products,
        language: edition.language.as_deref(),
        claim: Claim::Edition {
            campaign_id: &campaign.id,
            edition_id: &edition.id,
        },
    };
    let progress = edition_progress(
        db,
        &tenant.id,
        &edition.id,
        edition.language.as_deref(),
        shop_zone,
    )
    .await?;
    let slots = schedule.slots(cycle);
    let fallback = TimeZone::get(shop_zone).unwrap_or(TimeZone::UTC);
    let mut mailed = 0usize;
    for zone in &progress {
        if pass.spent() {
            break;
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
        let due = if passed >= instants.len() {
            // The zone's last slot has come: whoever it still owes.
            u32::MAX
        } else {
            // The counts cannot tell who left the list once mailed, so the
            // zone's whole is at least what is mailed or on the list now;
            // what that leaves out goes at the last slot. Every node of
            // the cluster takes its part of what is due.
            let (delivered, audience) = (zone.delivered.max(0) as u64, zone.audience.max(0) as u64);
            let due =
                quota(audience.max(delivered), passed, instants.len()).saturating_sub(delivered);
            u32::try_from(due.div_ceil(pass.walk.share.of)).unwrap_or(u32::MAX)
        };
        if due == 0 {
            continue;
        }
        let list = List::Zone {
            shop_zone,
            zone: &zone.zone,
        };
        mailed += out.walk(pass, list, due).await?.0;
    }
    Ok(mailed)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use evento::Evento;
    use timada_core::Key;
    use timada_newsletter::{CampaignDraft, RequestOutcome, RequestSubscription, SOURCE_FORM};
    use topcoat::context::CxTestBuilder;
    use topcoat::mail::{MailConfig, MemoryTransport, Transport};

    use super::*;
    use crate::mail::MailSender;
    use crate::mail_fault::fake_smtp;
    use crate::shop_mail::ShopMail;

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

    /// An in-memory store, and an in-memory read-model database.
    async fn store() -> (evento::Turso, Db) {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&store).await.unwrap();
        let db = Db::from(store.clone());
        (store, db)
    }

    async fn catch_up(store: &evento::Turso, db: &Db, keys: Arc<MemoryKeys>) {
        timada_catalog::read_model_subscription::<evento::Turso>(db.clone())
            .run_once(store)
            .await
            .unwrap();
        timada_newsletter::read_model_subscription::<evento::Turso>(
            db.clone(),
            Arc::new(MemoryRing(keys)),
        )
        .run_once(store)
        .await
        .unwrap();
    }

    async fn subscribe(store: &evento::Turso, keys: &MemoryKeys, email: &str) {
        subscribe_in(store, keys, email, None).await;
    }

    async fn subscribe_in(
        store: &evento::Turso,
        keys: &MemoryKeys,
        email: &str,
        time_zone: Option<&str>,
    ) {
        subscribe_to(store, TENANT, keys, email, time_zone).await;
    }

    async fn subscribe_to(
        store: &evento::Turso,
        tenant_id: &str,
        keys: &MemoryKeys,
        email: &str,
        time_zone: Option<&str>,
    ) {
        let newsletter = timada_newsletter::Command::new(store.clone(), tenant_id);
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
            .app_context(media())
            .build();
        (cx, platform, orders, news)
    }

    fn media() -> Arc<crate::media::Media> {
        Arc::new(crate::media::Media::new(
            &crate::config::MediaConfig {
                dir: std::env::temp_dir(),
                public_url: "https://media.example".to_owned(),
                tenant_quota_bytes: 0,
                reconcile_check_secs: 86_400,
            },
            None,
        ))
    }

    /// What one look lets a node alone send, for an hour.
    fn pass(limit: u32) -> Pass {
        Pass::new(Walk::new(Share::ALONE), limit, far())
    }

    /// The next look of the node `look` was, going on from its walk.
    fn next(look: Pass, limit: u32) -> Pass {
        Pass::new(look.walk, limit, far())
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(3600)
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
        let (store, db) = store().await;
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
        catch_up(&store, &db, keys.clone()).await;
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
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(2))
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
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(2))
                .await
                .unwrap(),
            0
        );
        assert!(platform.sent().is_empty());
        assert!(orders.sent().is_empty());
        assert!(transport.sent().is_empty());
        shop.marketing_server_id = Some("news".into());

        // A look sends up to its limit.
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(2))
                .await
                .unwrap(),
            2
        );
        // A walk from the top before the read model caught up (a node that
        // just started, or one that disagreed on who is up): the first two
        // claims are refused, nobody is mailed twice, the third goes.
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(2))
                .await
                .unwrap(),
            1
        );
        // The read model still owes them all: not finished yet.
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sending);
        catch_up(&store, &db, keys.clone()).await;
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(2))
                .await
                .unwrap(),
            0
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
        let (store, db) = store().await;
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
        catch_up(&store, &db, keys.clone()).await;
        let shop = shop();
        let (cx, _platform, _orders, transport) = mail(&shop);

        // The read model owes all three. Ada leaves (the read model does not
        // know yet); Grace's key is not on this node.
        let owed = undelivered_subscribers(&db, TENANT, &campaign, None, Share::ALONE, "", 10)
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
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(10))
                .await
                .unwrap(),
            1
        );
        assert_eq!(recipients(&transport), ["alan@example.com"]);
        catch_up(&store, &db, keys.clone()).await;
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(10))
                .await
                .unwrap(),
            0
        );
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sending, "Grace is still owed");
        let owed = undelivered_subscribers(&db, TENANT, &campaign, None, Share::ALONE, "", 10)
            .await
            .unwrap();
        assert_eq!(owed.len(), 1, "Ada is not owed any more: {owed:?}");
        assert_eq!(owed[0].email, "grace@example.com");

        // The key arrives: Grace is mailed, and the campaign is finished once
        // the read model owes nobody.
        keys.put(&grace.key_id, &grace_key).await.unwrap();
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(10))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            recipients(&transport),
            ["alan@example.com", "grace@example.com"]
        );
        catch_up(&store, &db, keys.clone()).await;
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(10))
                .await
                .unwrap(),
            0
        );
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);
        catch_up(&store, &db, keys.clone()).await;
        let row = timada_newsletter::campaign_row(&db, TENANT, &campaign)
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
        store: &evento::Turso,
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

        let (store, db) = store().await;
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
        catch_up(&store, &db, keys.clone()).await;
        assert!(open_editions(&db).await.unwrap().is_empty());

        // 9:00 in Kiritimati: open, but nobody's slot has come.
        open_due_editions(&store, &shop, &campaign, at("2030-01-07T19:00:00Z"))
            .await
            .unwrap();
        catch_up(&store, &db, keys.clone()).await;
        let [(_, edition)] = open_editions(&db).await.unwrap().try_into().unwrap();
        let sweep = async |moment: &str| {
            let mailed = send_edition_batch(
                &store,
                &db,
                &cx,
                &shop,
                &*keys,
                &edition,
                &mut pass(100),
                at(moment),
            )
            .await
            .unwrap();
            catch_up(&store, &db, keys.clone()).await;
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

        let (store, db) = store().await;
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
            catch_up(&store, &db, keys.clone()).await;
            for (_, edition) in open_editions(&db).await.unwrap() {
                send_edition_batch(
                    &store,
                    &db,
                    &cx,
                    &shop,
                    &*keys,
                    &edition,
                    &mut pass(100),
                    at(moment),
                )
                .await
                .unwrap();
                catch_up(&store, &db, keys.clone()).await;
                // Twice in the same slot: no second mail.
                send_edition_batch(
                    &store,
                    &db,
                    &cx,
                    &shop,
                    &*keys,
                    &edition,
                    &mut pass(100),
                    at(moment),
                )
                .await
                .unwrap();
                catch_up(&store, &db, keys.clone()).await;
            }
            assert_eq!(transport.sent().len(), editions);
        }
        assert_eq!(
            timada_newsletter::editions_of_campaign(&db, TENANT, &campaign, 10)
                .await
                .unwrap()
                .len(),
            2
        );

        // Past its end date, the windows closed: the campaign is done.
        let after = at("2030-01-12T00:00:00Z");
        for (_, edition) in open_editions(&db).await.unwrap() {
            send_edition_batch(
                &store,
                &db,
                &cx,
                &shop,
                &*keys,
                &edition,
                &mut pass(100),
                after,
            )
            .await
            .unwrap();
        }
        open_due_editions(&store, &shop, &campaign, after)
            .await
            .unwrap();
        catch_up(&store, &db, keys.clone()).await;
        assert!(open_editions(&db).await.unwrap().is_empty());
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);
    }

    #[tokio::test]
    async fn a_campaign_shows_the_products_just_in() {
        let (store, db) = store().await;
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
        catch_up(&store, &db, keys.clone()).await;
        let shop = shop();
        let (cx, _, _, transport) = mail(&shop);
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut pass(10))
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

    async fn sent_campaign(store: &evento::Turso, tenant_id: &str) -> String {
        let newsletter = timada_newsletter::Command::new(store.clone(), tenant_id);
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
        newsletter
            .send_campaign("user-alice", &campaign)
            .await
            .unwrap();
        campaign
    }

    /// A look goes on from the last subscriber the one before looked at (the
    /// node keeps where each walk stands from one look to the next), not
    /// from the top: one who joins meanwhile ahead of the walk waits for its
    /// next pass.
    #[tokio::test]
    async fn a_look_goes_on_from_where_the_last_one_stopped() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let shop_mail = shop_mail();
        let transport = MemoryTransport::new();
        shop_in_store(&store, TENANT, &shop_mail, Arc::new(transport.clone())).await;
        // Four addresses in the order of their subscribers' ids: the first
        // joins late.
        let mut by_id: Vec<(String, String)> = (0..4)
            .map(|n| {
                let email = format!("reader{n}@example.com");
                (timada_newsletter::subscriber_id(&email), email)
            })
            .collect();
        by_id.sort();
        let emails: Vec<String> = by_id.into_iter().map(|(_, email)| email).collect();
        for email in &emails[1..] {
            subscribe(&store, &keys, email).await;
        }
        let campaign = sent_campaign(&store, TENANT).await;
        catch_up(&store, &db, keys.clone()).await;
        // One mail a look.
        let sends = NewsletterSends::new(
            db.clone(),
            Evento::new(store.clone()),
            background(shop_mail),
            Arc::new(MemoryRing(keys.clone())),
            Arc::new(|| Share::ALONE),
            1,
            Duration::from_secs(60),
        );
        let now = crate::auth::now() as i64;
        let sorted = |emails: &[String]| {
            let mut emails = emails.to_vec();
            emails.sort();
            emails
        };

        assert_eq!(sends.sweep(now).await.unwrap(), 1);
        assert_eq!(recipients(&transport), [emails[1].clone()]);
        // Joined ahead of the walk: a walk from the top would mail them next.
        subscribe(&store, &keys, &emails[0]).await;
        catch_up(&store, &db, keys.clone()).await;
        assert_eq!(sends.sweep(now).await.unwrap(), 1);
        assert_eq!(sends.sweep(now).await.unwrap(), 1);
        assert_eq!(recipients(&transport), sorted(&emails[1..]));
        // The end of the list; the next pass, from the top, finds them.
        assert_eq!(sends.sweep(now).await.unwrap(), 0);
        assert_eq!(sends.sweep(now).await.unwrap(), 1);
        assert_eq!(recipients(&transport), sorted(&emails));
        catch_up(&store, &db, keys.clone()).await;
        assert_eq!(sends.sweep(now).await.unwrap(), 0);
        let state = timada_newsletter::Command::new(store.clone(), TENANT)
            .load_campaign(&campaign)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);
    }

    /// A node that is up but sends nothing (copying its store, catching up,
    /// its writes refused) holds its part back for a pass or two only: once
    /// a whole pass over its own part found nobody to mail while someone is
    /// still owed, a node walks everyone's.
    #[tokio::test]
    async fn a_node_that_sends_nothing_holds_nobody_back_for_good() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let emails: Vec<String> = (0..12).map(|n| format!("reader{n}@example.com")).collect();
        for email in &emails {
            subscribe(&store, &keys, email).await;
        }
        let campaign = sent_campaign(&store, TENANT).await;
        timada_orders::Command::new(store.clone(), TENANT)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
        catch_up(&store, &db, keys.clone()).await;
        let shop = shop();
        let (cx, _, _, transport) = mail(&shop);
        // Node 0 of two; node 1 never looks.
        let mine = Share { index: 0, of: 2 };
        let own = emails
            .iter()
            .filter(|email| mine.owns(&timada_newsletter::subscriber_id(email)))
            .count();
        assert!(own > 0 && own < emails.len());

        let mut look = Pass::new(Walk::new(mine), 100, far());
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut look)
                .await
                .unwrap(),
            own
        );
        catch_up(&store, &db, keys.clone()).await;
        // A whole pass over its own part finds nobody: the next pass walks
        // everyone's.
        let mut look = next(look, 100);
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut look)
                .await
                .unwrap(),
            0
        );
        let mut look = next(look, 100);
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut look)
                .await
                .unwrap(),
            emails.len() - own
        );
        catch_up(&store, &db, keys.clone()).await;
        let mut look = next(look, 100);
        send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut look)
            .await
            .unwrap();
        let state = timada_newsletter::Command::new(store.clone(), TENANT)
            .load_campaign(&campaign)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);
        let mut everyone = emails.clone();
        everyone.sort();
        assert_eq!(recipients(&transport), everyone);
    }

    /// Two nodes each mail their own part of the subscribers, side by side:
    /// together everyone, once, and the campaign is finished once nobody's
    /// part owes anyone.
    #[tokio::test]
    async fn two_nodes_mail_each_their_part_of_a_campaign() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let emails: Vec<String> = (0..12).map(|n| format!("reader{n}@example.com")).collect();
        for email in &emails {
            subscribe(&store, &keys, email).await;
        }
        let campaign = sent_campaign(&store, TENANT).await;
        timada_orders::Command::new(store.clone(), TENANT)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
        catch_up(&store, &db, keys.clone()).await;
        let shop = shop();
        let (cx, _, _, transport) = mail(&shop);
        let halves = [Share { index: 0, of: 2 }, Share { index: 1, of: 2 }];
        let part = |share: Share| {
            let mut part: Vec<String> = emails
                .iter()
                .filter(|email| share.owns(&timada_newsletter::subscriber_id(email)))
                .cloned()
                .collect();
            part.sort();
            part
        };
        let (first, second) = (part(halves[0]), part(halves[1]));
        assert!(!first.is_empty() && !second.is_empty());

        let mut node_0 = Pass::new(Walk::new(halves[0]), 100, far());
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut node_0)
                .await
                .unwrap(),
            first.len()
        );
        assert_eq!(recipients(&transport), first);
        // Node 1's part is still owed: not finished.
        catch_up(&store, &db, keys.clone()).await;
        let mut node_0 = next(node_0, 100);
        send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut node_0)
            .await
            .unwrap();
        let newsletter = timada_newsletter::Command::new(store.clone(), TENANT);
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sending);

        let mut node_1 = Pass::new(Walk::new(halves[1]), 100, far());
        assert_eq!(
            send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut node_1)
                .await
                .unwrap(),
            second.len()
        );
        let mut everyone = emails.clone();
        everyone.sort();
        assert_eq!(recipients(&transport), everyone);
        catch_up(&store, &db, keys.clone()).await;
        let mut node_0 = next(node_0, 100);
        send_batch(&store, &db, &cx, &shop, &*keys, &campaign, &mut node_0)
            .await
            .unwrap();
        let state = newsletter.load_campaign(&campaign).await.unwrap().unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);
        catch_up(&store, &db, keys.clone()).await;
        let row = timada_newsletter::campaign_row(&db, TENANT, &campaign)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((row.delivered, row.failed, row.skipped), (12, 0, 0));
    }

    /// What a slot owes a zone is the cluster's: each node mails its part
    /// of it, not the whole, and at the last slot whoever its part still
    /// owes.
    #[tokio::test]
    async fn the_nodes_share_what_a_slot_owes() {
        use timada_newsletter::schedule::{Cadence, Schedule};

        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let halves = [Share { index: 0, of: 2 }, Share { index: 1, of: 2 }];
        // Two Parisians in each node's part.
        let candidates: Vec<String> = (0..40).map(|n| format!("paris{n}@example.com")).collect();
        for share in halves {
            for email in candidates
                .iter()
                .filter(|email| share.owns(&timada_newsletter::subscriber_id(email)))
                .take(2)
            {
                subscribe_in(&store, &keys, email, Some("Europe/Paris")).await;
            }
        }
        timada_orders::Command::new(store.clone(), TENANT)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
        // Once, on Tuesday 8 January 2030, 9:00 and 9:30.
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
        open_due_editions(&store, &shop, &campaign, at("2030-01-07T19:00:00Z"))
            .await
            .unwrap();
        catch_up(&store, &db, keys.clone()).await;
        let [(_, edition)] = open_editions(&db).await.unwrap().try_into().unwrap();
        let look = async |share: Share, moment: &str| {
            send_edition_batch(
                &store,
                &db,
                &cx,
                &shop,
                &*keys,
                &edition,
                &mut Pass::new(Walk::new(share), 100, far()),
                at(moment),
            )
            .await
            .unwrap()
        };

        // 9:00 in Paris: half of the four, one from each node, both looking
        // before either's mail reached the read model.
        assert_eq!(look(halves[0], "2030-01-08T08:00:00Z").await, 1);
        assert_eq!(look(halves[1], "2030-01-08T08:00:00Z").await, 1);
        catch_up(&store, &db, keys.clone()).await;
        assert_eq!(look(halves[0], "2030-01-08T08:10:00Z").await, 0);
        // 9:30, the last slot: the rest of each part.
        assert_eq!(look(halves[0], "2030-01-08T08:30:00Z").await, 1);
        assert_eq!(look(halves[1], "2030-01-08T08:30:00Z").await, 1);
        assert_eq!(transport.sent().len(), 4);
        assert_eq!(recipients(&transport).len(), 4);
    }

    /// The shops' servers, any of which a test may stand in for.
    fn shop_mail() -> ShopMail {
        ShopMail::new(
            Some(crate::seal::Sealer::new(
                "mail",
                "a-secrets-key-of-thirty-two-chars!!",
            )),
            true,
        )
    }

    /// The background context the sweep mails from, with the shops'
    /// servers `shop_mail` holds.
    fn background(shop_mail: ShopMail) -> Arc<AppContext> {
        let mut app = AppContext::new();
        app.insert(
            MailConfig::builder()
                .transport(MemoryTransport::new())
                .build(),
        );
        app.insert(MailSender("Timada <no-reply@example.com>".parse().unwrap()));
        app.insert(shop_mail);
        app.insert(media());
        Arc::new(app)
    }

    /// A shop in the store whose marketing goes through its server `news`,
    /// which `transport` stands in for, with its storefront.
    async fn shop_in_store(
        store: &evento::Turso,
        tenant_id: &str,
        shop_mail: &ShopMail,
        transport: Arc<dyn Transport>,
    ) {
        evento::append(tenant_id)
            .original_version(0)
            .event(&timada_tenancy::aggregator::Created {
                slug: tenant_id.to_owned(),
                name: format!("Shop {tenant_id}"),
                created_by: "user-alice".to_owned(),
            })
            .event(&timada_tenancy::aggregator::MailServerAdded {
                server_id: "news".to_owned(),
                name: None,
                host: "localhost".to_owned(),
                port: 1025,
                security: timada_tenancy::MAIL_SECURITY_NONE.to_owned(),
                username: None,
                password_sealed: None,
                from_address: format!("news@{tenant_id}.test"),
                from_name: None,
                added_by: "user-alice".to_owned(),
            })
            .event(&timada_tenancy::aggregator::MailServersPicked {
                shop_mails: None,
                marketing: Some("news".to_owned()),
                picked_by: "user-alice".to_owned(),
            })
            .commit(store)
            .await
            .unwrap();
        let tenant = timada_tenancy::Command(Evento::new(store.clone()))
            .load_tenant(tenant_id)
            .await
            .unwrap()
            .unwrap();
        shop_mail.stand_in_for(&tenant, "news", transport);
        timada_orders::Command::new(store.clone(), tenant_id)
            .set_storefront_address("user-alice", Some(SHOP))
            .await
            .unwrap();
    }

    /// Each shop's mails go out in a lane of their own: a shop whose server
    /// is down costs that shop one subscriber's mail per look, and holds
    /// nobody else's back.
    #[tokio::test]
    async fn a_shop_whose_server_is_down_holds_back_nobody_else() {
        let (store, db) = store().await;
        let keys = Arc::new(MemoryKeys::default());
        let shop_mail = shop_mail();
        let up = MemoryTransport::new();
        shop_in_store(
            &store,
            "down",
            &shop_mail,
            Arc::new(fake_smtp::transport(fake_smtp::closed_port())),
        )
        .await;
        shop_in_store(&store, "up", &shop_mail, Arc::new(up.clone())).await;
        let mut campaigns = HashMap::new();
        for tenant_id in ["down", "up"] {
            for n in 0..3 {
                subscribe_to(
                    &store,
                    tenant_id,
                    &keys,
                    &format!("reader{n}@example.com"),
                    None,
                )
                .await;
            }
            campaigns.insert(tenant_id, sent_campaign(&store, tenant_id).await);
        }
        catch_up(&store, &db, keys.clone()).await;
        let sends = NewsletterSends::new(
            db.clone(),
            Evento::new(store.clone()),
            background(shop_mail),
            Arc::new(MemoryRing(keys.clone())),
            Arc::new(|| Share::ALONE),
            100,
            Duration::from_secs(60),
        );

        assert_eq!(sends.sweep(crate::auth::now() as i64).await.unwrap(), 3);
        assert_eq!(recipients(&up).len(), 3);
        catch_up(&store, &db, keys.clone()).await;
        let row = timada_newsletter::campaign_row(&db, "down", &campaigns["down"])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (row.delivered, row.failed),
            (0, 1),
            "one tried, then stopped"
        );
        assert_eq!(
            undelivered_subscribers(&db, "down", &campaigns["down"], None, Share::ALONE, "", 10)
                .await
                .unwrap()
                .len(),
            2
        );
        // The shop whose server took them all is done at the next look.
        sends.sweep(crate::auth::now() as i64).await.unwrap();
        let state = timada_newsletter::Command::new(store.clone(), "up")
            .load_campaign(&campaigns["up"])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.status, CampaignStatus::Sent);
    }
}
