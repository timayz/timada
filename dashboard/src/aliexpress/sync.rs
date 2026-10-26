//! Reads the shops' imported AliExpress products again on a timer: stock,
//! price by rule and availability follow AliExpress. Nothing in the event
//! store fires later, so every node sweeps the links due in the shared read
//! model; a read another node recorded in between wins (`record_sync` writes
//! against the version it started from). The same sweep first renews every
//! connected account's access a day before it lapses, whether or not the shop
//! has anything imported, and then reads the orders placed on AliExpress
//! again until they ship (see [`super::forward`]).

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use timada_aliexpress::{ForwardRow, LinkRow, forwards_due, links_due};
use tokio::task::JoinHandle;

use super::forward::CheckResult;
use super::{Aliexpress, SyncResult};
use crate::cluster::Executor;

/// The most links one sweep reads, over every shop.
const SWEEP_LIMIT: i64 = 500;

/// A pause between two reads of one shop, so a sweep never bursts against
/// AliExpress's limits.
const PACE: Duration = Duration::from_millis(200);

pub struct AliexpressSync {
    read_pool: SqlitePool,
    /// The store, every tenant's.
    store: Executor,
    aliexpress: Arc<Aliexpress>,
}

impl AliexpressSync {
    pub fn new(read_pool: SqlitePool, store: Executor, aliexpress: Arc<Aliexpress>) -> Arc<Self> {
        Arc::new(Self {
            read_pool,
            store,
            aliexpress,
        })
    }

    /// Sweeps every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(err) = self.sweep(crate::auth::now()).await {
                    tracing::error!(error = %err, "AliExpress products not swept");
                }
            }
        })
    }

    /// Renews the access that lapses within a day, then reads again every
    /// link last read longer ago than the configured period, shop by shop. A
    /// shop that fails is logged and the others still go. Returns how many
    /// links were brought in step.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        if !self.aliexpress.enabled() {
            return Ok(0);
        }
        self.renew_due(now).await?;
        let every = self.aliexpress.sync_every_secs();
        let before = now.saturating_sub(every) as i64;
        let due = links_due(&self.read_pool, before, SWEEP_LIMIT).await?;
        let mut synced = 0;
        for shop in due.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = &shop[0].tenant_id;
            match self.sweep_tenant(tenant_id, shop, now).await {
                Ok(count) => synced += count,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, error = %err, "AliExpress products not read")
                }
            }
        }
        self.check_forwards(now).await?;
        Ok(synced)
    }

    /// Reads the orders placed on AliExpress again, shop by shop; returns
    /// how many shop orders were fulfilled from them.
    pub async fn check_forwards(&self, now: u64) -> anyhow::Result<usize> {
        let due = forwards_due(&self.read_pool, SWEEP_LIMIT).await?;
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
        let Some(tenant) = self.aliexpress.active_tenant(tenant_id).await? else {
            return Ok(0);
        };
        if tenant.aliexpress.is_none() {
            return Ok(0);
        }
        let store = self.store.clone();
        let tenant = self.aliexpress.refresh_if_due(&store, tenant, now).await?;
        let mut fulfilled = 0;
        for (index, forward) in forwards.iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(PACE).await;
            }
            match self
                .aliexpress
                .check_forward(&store, &tenant, &forward.id, now)
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

    /// Renews the access of every connected shop due within the margin,
    /// the soonest first. The tenant's own state decides (the read model may
    /// lag), and one node per token renews it.
    async fn renew_due(&self, now: u64) -> anyhow::Result<()> {
        let before = now.saturating_add(timada_aliexpress::REFRESH_MARGIN_SECS) as i64;
        let due =
            timada_tenancy::aliexpress_access_due(&self.read_pool, before, now as i64, SWEEP_LIMIT)
                .await?;
        for tenant_id in &due {
            if let Err(err) = self.renew_tenant(tenant_id, now).await {
                tracing::error!(tenant = %tenant_id, error = %err, "AliExpress access not renewed");
            }
        }
        Ok(())
    }

    async fn renew_tenant(&self, tenant_id: &str, now: u64) -> anyhow::Result<()> {
        let Some(tenant) = self.aliexpress.active_tenant(tenant_id).await? else {
            return Ok(());
        };
        if tenant.aliexpress.is_none() {
            return Ok(());
        }
        self.aliexpress
            .refresh_if_due(&self.store, tenant, now)
            .await?;
        Ok(())
    }

    async fn sweep_tenant(
        &self,
        tenant_id: &str,
        links: &[LinkRow],
        now: u64,
    ) -> anyhow::Result<usize> {
        let Some(tenant) = timada_tenancy::Command(self.store.clone())
            .load_tenant(tenant_id)
            .await?
        else {
            return Ok(0);
        };
        if tenant.is_suspended() || tenant.aliexpress.is_none() {
            return Ok(0);
        }
        let store = self.store.clone();
        let tenant = self.aliexpress.refresh_if_due(&store, tenant, now).await?;
        let mut synced = 0;
        for (index, link) in links.iter().enumerate() {
            if index > 0 {
                tokio::time::sleep(PACE).await;
            }
            match self.aliexpress.sync_link(&store, &tenant, &link.id).await {
                Ok(SyncResult::Synced { .. }) => synced += 1,
                Ok(SyncResult::NotLinked) => break,
                Ok(SyncResult::Skipped | SyncResult::Failed(_)) => {}
                Err(err) => {
                    tracing::warn!(tenant = %tenant_id, link = %link.id, error = %err, "AliExpress product not read")
                }
            }
        }
        Ok(synced)
    }
}
