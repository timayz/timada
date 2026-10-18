//! Reads the shops' imported AliExpress products again on a timer: stock,
//! price by rule and availability follow AliExpress. Nothing in the event
//! store fires later, so every node sweeps the links due in the shared read
//! model; a read another node recorded in between wins (`record_sync` writes
//! against the version it started from). The same sweep renews an account's
//! access a day before it lapses.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use timada_aliexpress::{LinkRow, links_due};
use tokio::task::JoinHandle;

use super::{Aliexpress, SyncResult};
use crate::cluster::Executor;
use crate::tenants::Tenants;

/// The most links one sweep reads, over every shop.
const SWEEP_LIMIT: i64 = 500;

/// A pause between two reads of one shop, so a sweep never bursts against
/// AliExpress's limits.
const PACE: Duration = Duration::from_millis(200);

pub struct AliexpressSync {
    read_pool: SqlitePool,
    platform: Executor,
    tenants: Arc<Tenants>,
    aliexpress: Arc<Aliexpress>,
}

impl AliexpressSync {
    pub fn new(
        read_pool: SqlitePool,
        platform: Executor,
        tenants: Arc<Tenants>,
        aliexpress: Arc<Aliexpress>,
    ) -> Arc<Self> {
        Arc::new(Self {
            read_pool,
            platform,
            tenants,
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

    /// Reads again every link last read longer ago than the configured
    /// period, shop by shop. A shop that fails is logged and the others
    /// still go. Returns how many were brought in step.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        if !self.aliexpress.enabled() {
            return Ok(0);
        }
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
        Ok(synced)
    }

    async fn sweep_tenant(
        &self,
        tenant_id: &str,
        links: &[LinkRow],
        now: u64,
    ) -> anyhow::Result<usize> {
        let Some(tenant) = timada_tenancy::Command(self.platform.clone())
            .load_tenant(tenant_id)
            .await?
        else {
            return Ok(0);
        };
        if tenant.is_suspended() || tenant.aliexpress.is_none() {
            return Ok(0);
        }
        let lease = self.tenants.lease(&tenant).await?;
        let store = lease.executor().clone();
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
