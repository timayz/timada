//! Reads a cart's imported goods again just before it is priced for payment,
//! so the shopper pays what AliExpress asks now: a price that moved is refused
//! as `price_changed`, goods it ran out of as `out_of_stock`. Nothing here
//! refuses anything: the checks of the payment go by the catalog, which the
//! read brought in step, and the goods a read cannot reach are priced as last
//! read.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use timada_aliexpress::{ALIEXPRESS_SUPPLIER, forward_items, link_of_product};
use timada_orders::OrderLine;
use tokio::task::JoinSet;

use super::{Aliexpress, SyncMode, SyncResult};
use crate::cluster::Executor;
use crate::db::Db;
use crate::payments::SuppliedGoods;

/// How long a payment waits for the reads of its cart. Payments are answered
/// one at a time and shoppers wait: a slow AliExpress must not hold them up.
/// The reads carry on past it, and the next payment finds them done.
const READ_WAIT: Duration = Duration::from_secs(3);

/// How long payments go by the last reads once AliExpress did not answer in
/// time. Payments of every shop go through one queue, so an outage costs one
/// wait per minute on a node instead of one per payment.
const UNREADABLE_SECS: u64 = 60;

pub struct AliexpressGoods {
    aliexpress: Arc<Aliexpress>,
    /// The read models: which product is imported, when it was read.
    db: Db,
    store: Executor,
    read_wait: Duration,
    /// Until when (unix seconds) payments go by the last reads.
    unreadable_until: AtomicU64,
}

impl AliexpressGoods {
    pub fn new(aliexpress: Arc<Aliexpress>, db: Db, store: Executor) -> Self {
        Self {
            aliexpress,
            db,
            store,
            read_wait: READ_WAIT,
            unreadable_until: AtomicU64::new(0),
        }
    }

    /// The same, waiting `read_wait` for the reads.
    #[cfg(test)]
    pub(super) fn waiting(mut self, read_wait: Duration) -> Self {
        self.read_wait = read_wait;
        self
    }

    async fn read(&self, tenant_id: &str, product_ids: &[String], now: u64) -> anyhow::Result<()> {
        if !self.aliexpress.enabled() || now < self.unreadable_until.load(Ordering::Acquire) {
            return Ok(());
        }
        // A recent read is good for a payment: a shopper confirming a new
        // price finds the product read, and a product many are buying costs
        // one call per period on a node, however many pay.
        let fresh_secs = self.aliexpress.read_fresh_secs();
        let mut stale = Vec::new();
        for product_id in product_ids {
            let Some(link) = link_of_product(&self.db, tenant_id, product_id).await? else {
                continue;
            };
            let fresh = link
                .last_synced_at
                .is_some_and(|at| now.saturating_sub(at as u64) < fresh_secs);
            if !fresh && !stale.contains(&link.id) {
                stale.push(link.id);
            }
        }
        if stale.is_empty() {
            return Ok(());
        }
        let Some((tenant, company)) = self.aliexpress.connected_shop(tenant_id, now).await? else {
            return Ok(());
        };
        let mut reads = JoinSet::new();
        for link_id in stale {
            let (aliexpress, db, store) =
                (self.aliexpress.clone(), self.db.clone(), self.store.clone());
            let (tenant, company) = (tenant.clone(), company.clone());
            reads.spawn(async move {
                let read = aliexpress
                    .sync_link(&db, &store, &tenant, &company, &link_id, SyncMode::Routine)
                    .await;
                match read {
                    Ok(SyncResult::Failed(reason)) => {
                        tracing::warn!(tenant = %tenant.id, link = %link_id, %reason, "AliExpress product not read before a payment: priced as last read");
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(tenant = %tenant.id, link = %link_id, error = %err, "AliExpress product not read before a payment: priced as last read");
                    }
                }
            });
        }
        let waited = tokio::time::timeout(self.read_wait, async {
            while reads.join_next().await.is_some() {}
        })
        .await;
        if waited.is_err() {
            reads.detach_all();
            self.unreadable_until
                .store(now + UNREADABLE_SECS, Ordering::Release);
            tracing::warn!(tenant = %tenant_id, "AliExpress did not answer in time: payments go by the last reads for a minute");
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SuppliedGoods for AliexpressGoods {
    async fn read_again(&self, tenant_id: &str, product_ids: &[String]) {
        if let Err(err) = self.read(tenant_id, product_ids, crate::auth::now()).await {
            tracing::warn!(tenant = %tenant_id, error = %err, "AliExpress products not read before a payment: priced as last read");
        }
    }

    /// The lines the forwarder would buy on AliExpress, matched the way it
    /// matches them, whether or not the shop is connected right now: the
    /// merchant may buy them by hand.
    async fn supplier_of(
        &self,
        tenant_id: &str,
        lines: &[OrderLine],
    ) -> anyhow::Result<Option<String>> {
        let links = super::forward::order_links(&self.store, &self.db, tenant_id, lines).await?;
        Ok(
            (!forward_items(lines, &links).items.is_empty())
                .then(|| ALIEXPRESS_SUPPLIER.to_owned()),
        )
    }
}
