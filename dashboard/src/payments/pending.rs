//! Settles what an order or a cart asked of the provider and did not get an
//! answer to.
//!
//! The money of an order is taken, or given back, by whoever asked for it,
//! at once ([`settle`](super::settle)). A provider that could not be reached
//! then, a node that stopped in between, or a webhook that never came leaves
//! the order waiting: a capture, a cancellation or a refund asked and not
//! settled, a hold that lapsed with nobody told, an order of goods the shop
//! holds whose money nobody asked for. A payment a cart asked to give back
//! (`Cart::PaymentReturnRequested`) waits the same way. So every node
//! sweeps, on a timer, the orders and carts the read model says are in one
//! of those states for more than [`ASKED_SECS`], and settles each with the
//! provider. Two nodes doing it at once ask the provider twice, under the
//! same keys; its answer is recorded once.
//!
//! A shop that is suspended is skipped. One that stopped taking payments is
//! settled with the keys it keeps for that; one whose keys are forgotten
//! still settles what asks the provider nothing: the cancellation of an
//! order whose hold went back already. An order the provider would not
//! answer for is asked about again only after
//! [`RECHECK_SECS`](super::RECHECK_SECS).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use timada_orders::{OrderStatus, payments_due, returns_due};
use tokio::task::JoinHandle;

use super::settle::{self, CancelOutcome, CaptureOutcome};
use super::webhook::{Outcome, reconcile};
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
    /// The read models: the orders and carts waiting on the provider.
    db: Db,
    /// The store, every tenant's, read from this node's file: what an order
    /// or a cart asked of the provider is asked again under the same keys,
    /// and its answer recorded at the version read.
    store: Executor,
    payments: Arc<Payments>,
    /// The orders, and the payments carts give back, the provider did not
    /// answer for, and the unix seconds from which the sweep asks again.
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

    /// Settles the orders and the payments given back waiting on the
    /// provider at `now`, shop by shop; returns how many it settled. A shop
    /// that fails is logged and the others go on.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let asked_before = now.saturating_sub(ASKED_SECS) as i64;
        let due = payments_due(&self.db, now as i64, asked_before, ORDERS_PER_SWEEP).await?;
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
        let returning = returns_due(&self.db, asked_before, ORDERS_PER_SWEEP).await?;
        for carts in returning.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = carts[0].tenant_id.as_str();
            let ids: Vec<&str> = carts.iter().map(|row| row.id.as_str()).collect();
            match self.sweep_returns(tenant_id, &ids, now).await {
                Ok(count) => settled += count,
                Err(err) => {
                    tracing::error!(%tenant_id, error = %err, "a shop's payments to give back not swept");
                }
            }
        }
        lock(&self.recheck_at).retain(|_, at| *at > now);
        Ok(settled)
    }

    /// The tenant's provider account, unless it is suspended or took it away.
    async fn account(&self, tenant_id: &str) -> anyhow::Result<Option<Account>> {
        Ok(match self.payments.active_tenant(tenant_id).await? {
            Some(tenant) => self.payments.account_of(&tenant)?,
            None => None,
        })
    }

    /// Whether the sweep leaves `key` (an order, a payment) alone at `now`.
    fn left_alone(&self, key: &str, now: u64) -> bool {
        lock(&self.recheck_at).get(key).is_some_and(|at| *at > now)
    }

    async fn sweep_tenant(&self, tenant_id: &str, ids: &[&str], now: u64) -> anyhow::Result<usize> {
        let Some(tenant) = self.payments.active_tenant(tenant_id).await? else {
            return Ok(0);
        };
        // A shop without its provider account still settles what asks the
        // provider nothing: the cancellations of orders whose hold went back.
        let account = self.payments.account_of(&tenant)?;
        let orders = timada_orders::Command::new(self.store.clone(), tenant_id);
        let mut settled = 0;
        for order_id in ids {
            if self.left_alone(order_id, now) {
                continue;
            }
            if settle::settle_released_cancellation(&orders, order_id)
                .await?
                .is_some()
            {
                settled += 1;
                continue;
            }
            let Some(account) = &account else {
                continue;
            };
            let Some(order) = orders.load_order(order_id).await? else {
                continue;
            };
            let answered = if order.pending_cancellation.is_some() {
                settle::settle_cancellation(&self.payments, account, &orders, order_id).await?
                    == CancelOutcome::Cancelled
            } else if order.pending_capture.is_some() {
                settle::capture(&self.payments, account, &orders, order_id).await?
                    != CaptureOutcome::Unanswered
            } else if order.pending_refund.is_some() {
                settle::settle_refund(&self.payments, account, &orders, order_id).await?
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
                    account,
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

    /// Gives back the payments the tenant's carts asked to return, as the
    /// webhook would: the provider is asked how each stands, and the return
    /// is finished under the reason it was asked with.
    async fn sweep_returns(
        &self,
        tenant_id: &str,
        cart_ids: &[&str],
        now: u64,
    ) -> anyhow::Result<usize> {
        let Some(account) = self.account(tenant_id).await? else {
            return Ok(0);
        };
        let orders = timada_orders::Command::new(self.store.clone(), tenant_id);
        let mut settled = 0;
        for cart_id in cart_ids {
            let Some(cart) = orders.load_cart(cart_id).await? else {
                continue;
            };
            let returning = cart
                .requests
                .iter()
                .filter_map(|request| request.returning.as_ref());
            for pending in returning {
                let payment_id = pending.payment_id.as_str();
                if self.left_alone(payment_id, now) {
                    continue;
                }
                let status = self
                    .payments
                    .gateway
                    .payment_status(&account.secret_key, payment_id)
                    .await;
                let returned = match status {
                    Ok(status) => {
                        reconcile(
                            &self.payments,
                            &account,
                            tenant_id,
                            self.store.clone(),
                            cart_id,
                            &status,
                        )
                        .await
                    }
                    Err(err) => Err(err),
                };
                match returned {
                    Ok(Outcome::Returned { .. } | Outcome::NotReturned { .. }) => settled += 1,
                    Ok(outcome) => {
                        tracing::warn!(%tenant_id, cart = %cart_id, payment = %payment_id, ?outcome, "a payment to give back is not given back yet");
                        lock(&self.recheck_at).insert(payment_id.to_owned(), now + RECHECK_SECS);
                    }
                    Err(err) => {
                        tracing::warn!(%tenant_id, cart = %cart_id, payment = %payment_id, error = %err, "a payment to give back could not be now");
                        lock(&self.recheck_at).insert(payment_id.to_owned(), now + RECHECK_SECS);
                    }
                }
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
