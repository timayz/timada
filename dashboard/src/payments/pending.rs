//! Settles what an order asked of the provider and did not get an answer to.
//!
//! The money of an order is taken, or given back, by whoever asked for it,
//! at once ([`settle`](super::settle)). A provider that could not be reached
//! then, a node that stopped in between, or a webhook that never came leaves
//! the order waiting: a capture or a cancellation asked and not settled, a
//! hold that lapsed with nobody told, an order of goods the shop holds whose
//! money nobody asked for. So every node sweeps, on a timer, the orders the
//! read model says are in one of those states for more than [`ASKED_SECS`],
//! and settles each with the provider. Two nodes doing it at once ask the
//! provider twice; its answer is recorded once.
//!
//! A shop that is suspended or took its provider account away is skipped.
//! An order the provider would not answer for is asked about again only
//! after [`RECHECK_SECS`](super::RECHECK_SECS).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use timada_orders::{OrderStatus, payments_due};
use tokio::task::JoinHandle;

use super::settle::{self, CancelOutcome, CaptureOutcome};
use super::webhook::reconcile;
use super::{Account, DASHBOARD_ACTOR, PaymentState, Payments, RECHECK_SECS, capture};
use crate::cluster::Executor;
use crate::db::Db;

/// How long a request stands before the sweep settles it: whoever asked is
/// given that long to do it.
pub const ASKED_SECS: u64 = 60;

/// The most orders one sweep looks at, every shop together.
const ORDERS_PER_SWEEP: i64 = 500;

/// The guard, poisoned or not: nothing held under this lock is left half
/// written by a panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Unix seconds now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The sweep, with what it needs outside any request.
pub struct PendingPayments {
    /// The read models: the orders waiting on the provider.
    db: Db,
    /// The store, every tenant's.
    store: Executor,
    payments: Arc<Payments>,
    /// The orders the provider did not answer for, and the unix seconds from
    /// which the sweep asks again.
    recheck_at: Mutex<HashMap<String, u64>>,
}

impl PendingPayments {
    pub fn new(db: Db, store: Executor, payments: Arc<Payments>) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            payments,
            recheck_at: Mutex::default(),
        })
    }

    /// Sweeps every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(err) = self.sweep(now()).await {
                    tracing::error!(error = %err, "pending payments not swept");
                }
            }
        })
    }

    /// Settles the orders waiting on the provider at `now`, shop by shop;
    /// returns how many it settled. A shop that fails is logged and the
    /// others go on.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let due = payments_due(
            &self.db,
            now as i64,
            now.saturating_sub(ASKED_SECS) as i64,
            ORDERS_PER_SWEEP,
        )
        .await?;
        let mut settled = 0;
        for orders in due.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = orders[0].tenant_id.as_str();
            let ids: Vec<&str> = orders.iter().map(|row| row.id.as_str()).collect();
            match self.sweep_tenant(tenant_id, &ids, now).await {
                Ok(count) => settled += count,
                Err(err) => {
                    tracing::error!(%tenant_id, error = %err, "a shop's pending payments not swept");
                }
            }
        }
        lock(&self.recheck_at).retain(|_, at| *at > now);
        Ok(settled)
    }

    async fn sweep_tenant(&self, tenant_id: &str, ids: &[&str], now: u64) -> anyhow::Result<usize> {
        let account = match self.payments.active_tenant(tenant_id).await? {
            Some(tenant) => self.payments.account_of(&tenant)?,
            None => None,
        };
        let Some(account) = account else {
            return Ok(0);
        };
        let orders = timada_orders::Command::new(self.store.clone(), tenant_id);
        let mut settled = 0;
        for order_id in ids {
            let left_alone = lock(&self.recheck_at)
                .get(*order_id)
                .is_some_and(|at| *at > now);
            if left_alone {
                continue;
            }
            let Some(order) = orders.load_order(order_id).await? else {
                continue;
            };
            let answered = if order.pending_cancellation.is_some() {
                settle::settle_cancellation(&self.payments, &account, &orders, order_id).await?
                    == CancelOutcome::Cancelled
            } else if order.pending_capture.is_some() {
                settle::capture(&self.payments, &account, &orders, order_id).await?
                    != CaptureOutcome::Unanswered
            } else if order.status != OrderStatus::Authorized || order.released_at.is_some() {
                // Settled, or kept open without its hold for the merchant:
                // nothing to ask the provider.
                true
            } else if order
                .payment
                .capture_before
                .is_some_and(|before| before <= now)
            {
                self.read_hold(
                    &account,
                    tenant_id,
                    &order.cart_id,
                    &order.payment.payment_id,
                )
                .await?
            } else if order.supplier.is_none() && order.may_capture() {
                capture::take_payment(
                    &self.payments,
                    &self.store,
                    tenant_id,
                    order_id,
                    DASHBOARD_ACTOR,
                )
                .await?;
                true
            } else {
                true
            };
            if answered {
                settled += 1;
            } else {
                lock(&self.recheck_at).insert((*order_id).to_owned(), now + RECHECK_SECS);
            }
        }
        Ok(settled)
    }

    /// Asks the provider about a hold whose time ran out and records what it
    /// says: dropped, the order is cancelled. Whether it answered.
    async fn read_hold(
        &self,
        account: &Account,
        tenant_id: &str,
        cart_id: &str,
        payment_id: &str,
    ) -> anyhow::Result<bool> {
        let status = match self
            .payments
            .gateway
            .payment_status(&account.secret_key, payment_id)
            .await
        {
            Ok(status) => status,
            Err(err) => {
                tracing::warn!(%tenant_id, payment = %payment_id, error = %err, "a lapsed hold could not be read now");
                return Ok(false);
            }
        };
        if status.state == PaymentState::Authorized {
            // Still held past what the card said: asked again later.
            return Ok(false);
        }
        reconcile(
            &self.payments,
            account,
            tenant_id,
            self.store.clone(),
            cart_id,
            &status,
        )
        .await?;
        Ok(true)
    }
}
