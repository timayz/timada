//! Reads what Merchant Center says of the connected shops' products, once
//! a day each and when a member asks: where each product stands for
//! Shopping ads and for the free listings, with Google's reasons, and what
//! Google holds against the account.
//!
//! Nothing in the event store fires later, so every node sweeps, on a
//! timer, the shops the read model says are due, each node those of its
//! part of the shops ([`Share`]). The shop's own streams decide: its
//! connection says whether Merchant Center can be read and which account,
//! and its watch from when a pass is due. Two nodes that disagree for a
//! moment on who is up both ask, and a read that says the same twice is
//! recorded once.
//!
//! A pass lists every product of the account, a thousand a question, then
//! asks what is held against the account. Google sets how many questions
//! an account takes in a day from how many products it holds (about twice
//! a full listing), so a daily read and a few asked for fit. What moved
//! since the read before is recorded on the shop's feed
//! (`timada-google/MerchantFeed`), which also says what turned for the
//! worse: the owners are told from there (`crate::merchant_notices`). A
//! pass that fails is recorded with its reason and tried again later,
//! further apart each time.

use std::sync::Arc;
use std::time::Duration;

use timada_core::Share;
use timada_google::{ACCESS_MERCHANT_CENTER, Command, DueMerchant, merchant_due};
use tokio::task::JoinHandle;

use crate::cluster::Executor;
use crate::db::Db;
use crate::google::{Google, GoogleFault};

/// Who the reads and the passes say recorded them.
pub const READER: &str = "timada-dashboard";

/// How many due shops one read takes.
const PAGE: i64 = 200;

/// How a read of a shop's account went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// Nothing to read with: no connection that covers Merchant Center, no
    /// account chosen, a lapsed grant, a suspended shop.
    Nothing,
    /// Read: how many products, and whether something moved.
    Done { products: usize, moved: bool },
    /// Google gave nothing, in its words: recorded on the watch.
    Failed(String),
}

/// Reads Merchant Center for one shop now and records what it says,
/// whatever its watch says of when: the daily pass checks that first, a
/// member who asks does not wait.
pub async fn read_shop<E: evento::Executor + Clone>(
    google: &Google,
    store: &E,
    tenant_id: &str,
) -> anyhow::Result<Read> {
    let command = Command(store.clone());
    let Some(connection) = command.load_connection(tenant_id).await? else {
        return Ok(Read::Nothing);
    };
    let Some(account) = connection
        .merchant
        .clone()
        .filter(|_| connection.reads(ACCESS_MERCHANT_CENTER))
    else {
        return Ok(Read::Nothing);
    };
    let shop = timada_tenancy::Command(store.clone())
        .load_tenant(tenant_id)
        .await?;
    if !shop.is_some_and(|shop| !shop.is_suspended()) {
        return Ok(Read::Nothing);
    }
    let account_id = account.account_id.as_str();
    match google.merchant_report(store, &connection, account_id).await {
        Ok(report) => {
            let products = report.products.len();
            let recorded = command
                .record_merchant_read(READER, tenant_id, account_id, report)
                .await?;
            command
                .record_merchant_check(READER, tenant_id, account_id, products as u32)
                .await?;
            tracing::debug!(tenant = %tenant_id, products, moved = recorded.written, "Merchant Center read");
            Ok(Read::Done {
                products,
                moved: recorded.written,
            })
        }
        // Recorded on the connection, or nothing to read with.
        Err(GoogleFault::ConnectAgain | GoogleFault::Off) => Ok(Read::Nothing),
        Err(fault) => {
            tracing::warn!(tenant = %tenant_id, %fault, "Merchant Center gave nothing");
            let reason = fault.to_string();
            command
                .record_merchant_failure(READER, tenant_id, account_id, &reason)
                .await?;
            Ok(Read::Failed(reason))
        }
    }
}

/// The sweep, with what it needs outside any request.
pub struct MerchantPasses {
    /// The read model: the shops due.
    db: Db,
    /// The store, read from this node's file: the connections and the
    /// watches decide, the read model only says where to look.
    store: Executor,
    /// This node's place among the nodes up, as of now: which shops it
    /// reads for.
    place: Arc<dyn Fn() -> Share + Send + Sync>,
    google: Google,
}

impl MerchantPasses {
    pub fn new(
        db: Db,
        store: Executor,
        place: Arc<dyn Fn() -> Share + Send + Sync>,
        google: Google,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            place,
            google,
        })
    }

    /// Sweeps every `every`, the first time right away. Nothing is swept
    /// while no Google application is set.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            if !self.google.enabled() {
                return;
            }
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                match self.sweep(crate::auth::now()).await {
                    Ok(0) => {}
                    Ok(shops) => tracing::info!(shops, "Merchant Center read"),
                    Err(err) => tracing::error!(error = %err, "Merchant Center not swept"),
                }
            }
        })
    }

    /// Reads Merchant Center for the shops due at `now` (unix seconds)
    /// among this node's part; returns how many were read. A shop that
    /// fails is logged and the others go on.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let share = (self.place)();
        let mut read = 0;
        let mut after = String::new();
        loop {
            let due = merchant_due(&self.db, now as i64, &after, PAGE).await?;
            for row in due.iter().filter(|row| share.owns(&row.tenant_id)) {
                match self.run(row, now).await {
                    Ok(true) => read += 1,
                    Ok(false) => {}
                    Err(err) => {
                        tracing::error!(tenant = %row.tenant_id, error = %err, "a shop's Merchant Center not read");
                    }
                }
            }
            match due.last() {
                Some(last) if due.len() as i64 == PAGE => after = last.tenant_id.clone(),
                _ => return Ok(read),
            }
        }
    }

    /// One shop's pass, if its streams say it is due; whether it was read.
    async fn run(&self, due: &DueMerchant, now: u64) -> anyhow::Result<bool> {
        let tenant_id = due.tenant_id.as_str();
        let command = Command(self.store.clone());
        // The write models decide; the read model only said where to look.
        let account = command
            .load_connection(tenant_id)
            .await?
            .and_then(|connection| connection.merchant);
        let Some(account) = account else {
            return Ok(false);
        };
        let watch = command
            .load_merchant_watch(tenant_id)
            .await?
            .filter(|watch| watch.account_id == account.account_id);
        if watch.is_some_and(|watch| now < watch.next_at) {
            return Ok(false);
        }
        let read = read_shop(&self.google, &self.store, tenant_id).await?;
        Ok(matches!(read, Read::Done { .. }))
    }
}

#[cfg(test)]
mod tests;
