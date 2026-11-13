//! Keeps the shops' imported AliExpress products in step without a timer per
//! product: nothing in the event store fires later, and a platform with more
//! products than its app may read in a day has to choose. So each node goes
//! through the links due of its own share in the read model, the most useful
//! first, and asks for each with a `SyncRequested` (see [`super::subscription`]
//! for the read) as fast as the day's budget of calls allows ([`super::budget`]).
//! What sales spend comes out of it, so the sweep gets what they leave.
//!
//! The open orders of this node's share placed on AliExpress that no push came
//! for in a while (see [`super::forward`], [`super::webhook`]), and the parcels
//! of the orders fulfilled from AliExpress, for the shopper's order page, are
//! read the same way, each from a lane of the budget of its own
//! ([`Lane`]): a season's parcels never take the products' calls, nor the
//! products the orders'.
//!
//! On a timer of its own, the sweep renews every connected company's access a
//! day before it lapses, whether or not its shops have anything imported,
//! records what this node placed on AliExpress while the store refused the
//! record, and says of the orders of this node's share left being placed that
//! AliExpress may have them. Nothing there waits for the budget, so a lane
//! short of calls never holds a renewal up.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use timada_aliexpress::{
    ALIEXPRESS_SUPPLIER, AliexpressError, FORWARD_STALL_SECS, FORWARD_WATCH_SECS, ForwardRow,
    ForwardStatus, forget_unrecorded, forwards_due, forwards_stalled, links_due, note_forward_read,
    note_parcels_delivered, note_parcels_followed, note_tried, note_unrecorded_try, parcels_due,
    unrecorded,
};
use tokio::task::JoinHandle;

use super::budget::Lane;
use super::forward::{CheckResult, MAY_HAVE_PLACED};

use super::{ALIEXPRESS_ACTOR, Aliexpress};
use crate::cluster::Executor;

/// The most accounts or orders one pass of the timer goes through, over
/// every shop.
const SWEEP_LIMIT: i64 = 500;

/// A pause between two reads of one shop's orders, so a sweep never bursts
/// against AliExpress's limits.
const PACE: Duration = Duration::from_millis(200);

/// How long what a shopper wanted makes a product come first: a cart left for a
/// while may still be paid for, and what sold lately is likely to sell again.
const WANTED_SECS: u64 = 30 * 24 * 60 * 60;

/// The longest a lane sleeps when nothing is due: a product may become due
/// at any time, and looking for that costs one query.
const LONGEST_IDLE: Duration = Duration::from_secs(60);

/// How many periods a parcel that stays still waits at most before it is
/// asked about again: one at the door's step waits a period, one that has
/// not moved for a week waits this many.
pub const PARCEL_STILL_PERIODS: u64 = 8;

pub struct AliexpressSync {
    /// The read models: the imports, the forwards, the companies' access.
    db: Db,
    /// The store, every tenant's, read from this node's file: what a read of
    /// an item, an order or a parcel brings is recorded at the version it
    /// started from, and a token is renewed by the node that claims it.
    store: Executor,
    aliexpress: Arc<Aliexpress>,
    /// How often the timer runs, and how soon an order with every parcel
    /// known is read again.
    every: Duration,
}

impl AliexpressSync {
    pub fn new(db: Db, store: Executor, aliexpress: Arc<Aliexpress>, every: Duration) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            aliexpress,
            every,
        })
    }

    /// Sweeps every `every`, the first time right away, and meanwhile reads
    /// what is due in each lane as the budget allows.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let idle = self.every.min(LONGEST_IDLE);
            tokio::join!(
                self.sweep_every(),
                self.drip(Lane::Products, idle),
                self.drip(Lane::Orders, idle),
                self.drip(Lane::Parcels, idle),
            );
        })
    }

    async fn sweep_every(&self) {
        let mut tick = tokio::time::interval(self.every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(err) = self.sweep(crate::auth::now()).await {
                tracing::error!(error = %err, "AliExpress accounts and orders not swept");
            }
        }
    }

    /// Goes through what is due in `lane`, a batch at a time, the budget
    /// giving the pace; sleeps `idle` when nothing is.
    async fn drip(&self, lane: Lane, idle: Duration) {
        if !self.aliexpress.enabled() || !self.aliexpress.budget().spends() {
            return;
        }
        loop {
            let now = crate::auth::now();
            let went_through = match lane {
                Lane::Products => self.ask_due(now).await,
                Lane::Orders => self.check_forwards(now).await,
                Lane::Parcels => self.follow_parcels(now).await,
            };
            match went_through {
                Ok(0) => tokio::time::sleep(idle).await,
                Ok(_) => {}
                Err(err) => {
                    tracing::error!(?lane, error = %err, "AliExpress not read");
                    tokio::time::sleep(idle).await;
                }
            }
        }
    }

    /// Goes through the links of this node's share due at `now`, the budget
    /// giving the pace, and asks for each to be read; how many it went through.
    /// A shop that cannot read now is passed over until the next period, like
    /// one read, or its products would take every turn it gets.
    pub async fn ask_due(&self, now: u64) -> anyhow::Result<usize> {
        let share = self.aliexpress.share();
        let budget = self.aliexpress.budget();
        let due = links_due(
            &self.db,
            now.saturating_sub(self.aliexpress.sync_every_secs()) as i64,
            now.saturating_sub(WANTED_SECS) as i64,
            share,
            budget.batch(Lane::Products, share),
        )
        .await?;
        let mut connected: HashMap<&str, bool> = HashMap::new();
        for link in &due {
            let tenant_id = link.tenant_id.as_str();
            let can_read = match connected.get(tenant_id) {
                Some(can_read) => *can_read,
                None => {
                    let can_read = self
                        .aliexpress
                        .connected_shop(tenant_id, now)
                        .await?
                        .is_some();
                    connected.insert(tenant_id, can_read);
                    can_read
                }
            };
            if can_read {
                budget
                    .ready(Lane::Products, || self.aliexpress.share())
                    .await;
                let links = timada_aliexpress::Command::new(self.store.clone(), tenant_id);
                if let Err(err) = links.request_sync(ALIEXPRESS_ACTOR, &link.id).await {
                    tracing::warn!(tenant = %tenant_id, link = %link.id, error = %err, "AliExpress product not asked for");
                    continue;
                }
            }
            note_tried(&self.db, tenant_id, &link.id, now).await?;
        }
        Ok(due.len())
    }

    /// Renews the access that lapses within a day, records what this node
    /// placed on AliExpress and could not record then, and says of the
    /// orders of this node's share left being placed that AliExpress may
    /// have them. Nothing here asks AliExpress but the renewals, which never
    /// wait.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<()> {
        if !self.aliexpress.enabled() {
            return Ok(());
        }
        self.renew_due(now).await?;
        self.record_unrecorded().await?;
        self.settle_stalled(now).await?;
        Ok(())
    }

    /// Asks AliExpress where the parcels of this node's share of the orders
    /// fulfilled from it are, those due at `now` ([`parcels_due`]), the
    /// budget giving the pace; how many forwards it went through. The ones
    /// followed to the door are asked about no more.
    pub async fn follow_parcels(&self, now: u64) -> anyhow::Result<usize> {
        let share = self.aliexpress.share();
        let budget = self.aliexpress.budget();
        let every = self.aliexpress.parcel_read_every_secs();
        let due = parcels_due(
            &self.db,
            now as i64,
            every as i64,
            every.saturating_mul(PARCEL_STILL_PERIODS) as i64,
            now.saturating_sub(FORWARD_WATCH_SECS) as i64,
            share,
            budget.batch(Lane::Parcels, share),
        )
        .await?;
        for shop in due.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = &shop[0].tenant_id;
            // A shop that cannot read is passed over until the next period,
            // like one read.
            let Some((tenant, company)) = self.aliexpress.connected_shop(tenant_id, now).await?
            else {
                for forward in shop {
                    note_parcels_followed(&self.db, tenant_id, &forward.id, now, false).await?;
                }
                continue;
            };
            for (index, forward) in shop.iter().enumerate() {
                if index > 0 {
                    tokio::time::sleep(PACE).await;
                }
                budget
                    .ready_for(
                        Lane::Parcels,
                        forward.tracking.split_whitespace().count(),
                        || self.aliexpress.share(),
                    )
                    .await;
                match self
                    .aliexpress
                    .follow_parcels(&self.store, &tenant, &company, &forward.id)
                    .await
                {
                    Ok(followed) => {
                        note_parcels_followed(
                            &self.db,
                            tenant_id,
                            &forward.id,
                            now,
                            followed.changed > 0,
                        )
                        .await?;
                        if followed.over {
                            note_parcels_delivered(&self.db, tenant_id, &forward.id, now).await?;
                        }
                    }
                    Err(err) => {
                        tracing::error!(tenant = %tenant_id, forward = %forward.id, error = %err, "AliExpress parcels not followed");
                        note_parcels_followed(&self.db, tenant_id, &forward.id, now, false).await?;
                    }
                }
            }
        }
        Ok(due.len())
    }

    /// Records what this node placed on AliExpress while the store would not
    /// take the record ([`keep_unrecorded`](timada_aliexpress::keep_unrecorded)),
    /// however long ago: a forward given up as stalled meanwhile is placed
    /// after all, and its capture asked. Returns how many it recorded; one
    /// still refused stays for the next sweep.
    pub async fn record_unrecorded(&self) -> anyhow::Result<usize> {
        let mut recorded = 0;
        for row in unrecorded(&self.db, SWEEP_LIMIT).await? {
            let placement = match row.placement() {
                Ok(placement) => placement,
                Err(err) => {
                    tracing::error!(tenant = %row.tenant_id, order = %row.order_id, forward = %row.forward_id, placement = %row.placement, error = %err, "a placement kept on this node cannot be read");
                    continue;
                }
            };
            let forwards = timada_aliexpress::Command::new(self.store.clone(), &row.tenant_id);
            match forwards
                .record_placed(ALIEXPRESS_ACTOR, &row.forward_id, placement.clone())
                .await
            {
                Ok(_) => {
                    tracing::info!(tenant = %row.tenant_id, order = %row.order_id, forward = %row.forward_id, ae_orders = ?placement.ae_order_ids, "order placed on AliExpress recorded at last");
                    forget_unrecorded(&self.db, &row).await?;
                    recorded += 1;
                }
                // On the forward, one of its numbers held for another: told
                // once, nothing a later try changes.
                Err(err @ AliexpressError::ClaimedElsewhere { .. }) => {
                    tracing::error!(tenant = %row.tenant_id, order = %row.order_id, forward = %row.forward_id, ae_orders = ?placement.ae_order_ids, error = %err, "an AliExpress order of this forward is recorded for another one");
                    forget_unrecorded(&self.db, &row).await?;
                    recorded += 1;
                }
                Err(err) => {
                    tracing::warn!(tenant = %row.tenant_id, order = %row.order_id, forward = %row.forward_id, ae_orders = ?placement.ae_order_ids, tries = row.tries + 1, error = %err, "order placed on AliExpress still not recorded");
                    note_unrecorded_try(&self.db, &row, &err.to_string()).await?;
                }
            }
        }
        Ok(recorded)
    }

    /// Fails each forward of this node's share taken on and not heard of for
    /// [`FORWARD_STALL_SECS`]: its node stopped, or could not record what
    /// AliExpress answered, so AliExpress may have the order and it is never
    /// placed again on its own ([`MAY_HAVE_PLACED`]). The shop's order is told
    /// first that its supplier may have it, so a later attempt AliExpress
    /// refuses keeps its fence. Returns how many it failed. The forward's own
    /// state decides; the read model only says which to look at.
    pub async fn settle_stalled(&self, now: u64) -> anyhow::Result<usize> {
        let before = now.saturating_sub(FORWARD_STALL_SECS) as i64;
        let stalled =
            forwards_stalled(&self.db, before, self.aliexpress.share(), SWEEP_LIMIT).await?;
        let mut failed = 0;
        for row in &stalled {
            let forwards = timada_aliexpress::Command::new(self.store.clone(), &row.tenant_id);
            let still = forwards.load_forward_by_id(&row.id).await.map(|forward| {
                forward.is_some_and(|forward| {
                    forward.status == ForwardStatus::Started
                        && now.saturating_sub(forward.updated_at) > FORWARD_STALL_SECS
                })
            });
            match still {
                Ok(true) => {}
                Ok(false) => continue,
                Err(err) => {
                    tracing::error!(tenant = %row.tenant_id, forward = %row.id, error = %err, "a stalled AliExpress order not settled");
                    continue;
                }
            }
            if let Err(err) = timada_orders::Command::new(self.store.clone(), &row.tenant_id)
                .record_supplier_uncertainty(
                    ALIEXPRESS_ACTOR,
                    &row.order_id,
                    ALIEXPRESS_SUPPLIER,
                    MAY_HAVE_PLACED,
                )
                .await
            {
                tracing::error!(tenant = %row.tenant_id, order = %row.order_id, forward = %row.id, error = %err, "a stalled AliExpress order not settled: its order not told");
                continue;
            }
            match forwards
                .fail_stalled_forward(ALIEXPRESS_ACTOR, &row.id, MAY_HAVE_PLACED, now)
                .await
            {
                Ok(true) => {
                    tracing::warn!(tenant = %row.tenant_id, order = %row.order_id, forward = %row.id, "placing an order on AliExpress never said how it went: the merchant looks there");
                    failed += 1;
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::error!(tenant = %row.tenant_id, forward = %row.id, error = %err, "a stalled AliExpress order not settled");
                }
            }
        }
        Ok(failed)
    }

    /// Reads the orders of this node's share placed on AliExpress that are due
    /// at `now` ([`forwards_due`]), shop by shop, the budget giving the pace;
    /// how many it went through.
    pub async fn check_forwards(&self, now: u64) -> anyhow::Result<usize> {
        let share = self.aliexpress.share();
        let before = now.saturating_sub(self.aliexpress.order_read_every_secs()) as i64;
        let settling_before = now.saturating_sub(self.every.as_secs()) as i64;
        let due = forwards_due(
            &self.db,
            before,
            settling_before,
            share,
            self.aliexpress.budget().batch(Lane::Orders, share),
        )
        .await?;
        for shop in due.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = &shop[0].tenant_id;
            if let Err(err) = self.check_tenant(tenant_id, shop, now).await {
                tracing::error!(tenant = %tenant_id, error = %err, "AliExpress orders not read")
            }
        }
        Ok(due.len())
    }

    async fn check_tenant(
        &self,
        tenant_id: &str,
        forwards: &[ForwardRow],
        now: u64,
    ) -> anyhow::Result<()> {
        // A shop that cannot read is passed over until the next period, like one
        // read, or its orders would take every turn it gets.
        let Some((tenant, company)) = self.aliexpress.connected_shop(tenant_id, now).await? else {
            for forward in forwards {
                note_forward_read(&self.db, tenant_id, &forward.id, now).await?;
            }
            return Ok(());
        };
        let store = self.store.clone();
        let budget = self.aliexpress.budget();
        for (index, forward) in forwards.iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(PACE).await;
            }
            budget
                .ready_for(
                    Lane::Orders,
                    forward.ae_order_ids.split_whitespace().count(),
                    || self.aliexpress.share(),
                )
                .await;
            match self
                .aliexpress
                .check_forward(&store, &tenant, &company, &forward.id, None, now)
                .await
            {
                Ok(CheckResult::Checked { fulfilled: true }) => {
                    // Its parcels were asked about with the fulfilment.
                    note_parcels_followed(&self.db, tenant_id, &forward.id, now, true).await?;
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(tenant = %tenant_id, forward = %forward.id, error = %err, "AliExpress order not read")
                }
            }
            note_forward_read(&self.db, tenant_id, &forward.id, now).await?;
        }
        Ok(())
    }

    /// Renews the access of every connected company due within the margin,
    /// the soonest first. The company's own state decides (the read model
    /// may lag), and one node per token renews it.
    async fn renew_due(&self, now: u64) -> anyhow::Result<()> {
        let before = now.saturating_add(timada_aliexpress::REFRESH_MARGIN_SECS) as i64;
        let due =
            timada_tenancy::aliexpress_accounts_due(&self.db, before, now as i64, SWEEP_LIMIT)
                .await?;
        for company_id in &due {
            if let Err(err) = self.renew_company(company_id, now).await {
                tracing::error!(company = %company_id, error = %err, "AliExpress access not renewed");
            }
        }
        Ok(())
    }

    async fn renew_company(&self, company_id: &str, now: u64) -> anyhow::Result<()> {
        let Some(company) = timada_tenancy::Command(self.store.clone())
            .load_company(company_id)
            .await?
        else {
            return Ok(());
        };
        self.aliexpress.refresh_if_due(company, now).await?;
        Ok(())
    }
}
