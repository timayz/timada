//! Keeps the shops' imported AliExpress products in step without a timer per
//! product: nothing in the event store fires later, and a platform with more
//! products than its app may read in a day has to choose. So each node goes
//! through the links due of its own share in the read model, the most useful
//! first, and asks for each with a `SyncRequested` (see [`super::subscription`]
//! for the read) as fast as the day's budget of calls allows ([`super::budget`]).
//! What sales spend comes out of it, so the sweep gets what they leave.
//!
//! On a timer of its own, the sweep renews every connected company's access a
//! day before it lapses, whether or not its shops have anything imported, and
//! reads the orders placed on AliExpress again until they ship (see
//! [`super::forward`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use timada_aliexpress::{ForwardRow, forwards_due, links_due, note_tried};
use tokio::task::JoinHandle;

use super::forward::CheckResult;

use super::{ALIEXPRESS_ACTOR, Aliexpress};
use crate::cluster::Executor;

/// The most accounts or orders one sweep goes through, over every shop.
const SWEEP_LIMIT: i64 = 500;

/// A pause between two reads of one shop's orders, so a sweep never bursts
/// against AliExpress's limits.
const PACE: Duration = Duration::from_millis(200);

/// How long what a shopper wanted makes a product come first: a cart left for a
/// while may still be paid for, and what sold lately is likely to sell again.
const WANTED_SECS: u64 = 30 * 24 * 60 * 60;

/// The longest the drip sleeps when nothing is due: a product may become due
/// at any time, and looking for that costs one query.
const LONGEST_IDLE: Duration = Duration::from_secs(60);

pub struct AliexpressSync {
    /// The read models: the imports, the forwards, the companies' access.
    db: Db,
    /// The store, every tenant's.
    store: Executor,
    aliexpress: Arc<Aliexpress>,
}

impl AliexpressSync {
    pub fn new(db: Db, store: Executor, aliexpress: Arc<Aliexpress>) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            aliexpress,
        })
    }

    /// Sweeps every `every`, the first time right away, and meanwhile asks for
    /// the products due as the budget allows.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            tokio::join!(self.sweep_every(every), self.drip(every.min(LONGEST_IDLE)));
        })
    }

    async fn sweep_every(&self, every: Duration) {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(err) = self.sweep(crate::auth::now()).await {
                tracing::error!(error = %err, "AliExpress accounts and orders not swept");
            }
        }
    }

    async fn drip(&self, idle: Duration) {
        if !self.aliexpress.enabled() || !self.aliexpress.budget().spends() {
            return;
        }
        loop {
            match self.ask_due(crate::auth::now()).await {
                Ok(0) => tokio::time::sleep(idle).await,
                Ok(_) => {}
                Err(err) => {
                    tracing::error!(error = %err, "AliExpress products not asked for");
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
            budget.batch(share),
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
                budget.ready(|| self.aliexpress.share()).await;
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

    /// Renews the access that lapses within a day, then reads the open orders
    /// again.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<()> {
        if !self.aliexpress.enabled() {
            return Ok(());
        }
        self.renew_due(now).await?;
        self.check_forwards(now).await?;
        Ok(())
    }

    /// Reads the orders placed on AliExpress again, shop by shop; returns
    /// how many shop orders were fulfilled from them.
    pub async fn check_forwards(&self, now: u64) -> anyhow::Result<usize> {
        let due = forwards_due(&self.db, SWEEP_LIMIT).await?;
        let mut fulfilled = 0;
        for shop in due.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = &shop[0].tenant_id;
            match self.check_tenant(tenant_id, shop, now).await {
                Ok(count) => fulfilled += count,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, error = %err, "AliExpress orders not read")
                }
            }
        }
        Ok(fulfilled)
    }

    async fn check_tenant(
        &self,
        tenant_id: &str,
        forwards: &[ForwardRow],
        now: u64,
    ) -> anyhow::Result<usize> {
        let Some((tenant, company)) = self.aliexpress.connected_shop(tenant_id, now).await? else {
            return Ok(0);
        };
        let store = self.store.clone();
        let mut fulfilled = 0;
        for (index, forward) in forwards.iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(PACE).await;
            }
            match self
                .aliexpress
                .check_forward(&store, &tenant, &company, &forward.id, now)
                .await
            {
                Ok(CheckResult::Checked { fulfilled: true }) => fulfilled += 1,
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(tenant = %tenant_id, forward = %forward.id, error = %err, "AliExpress order not read")
                }
            }
        }
        Ok(fulfilled)
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
