//! Closes the card forms of checkouts whose holds lapsed.
//!
//! A payment is opened with the provider once the goods and the discount's
//! use are held for the checkout, for [`RESERVATION_SECS`]. A shopper who
//! leaves the card form open past that and confirms later pays for goods
//! that may be gone, and the money then goes back ([`webhook`]) at the cost
//! of the provider's fee. So every node sweeps, on a timer, the carts whose
//! payment is still open past the hold, and cancels the payment with the
//! provider while nobody confirmed it (`Cart::PaymentCancelled`): the
//! shopper asks to pay again, and nothing was charged. A payment the
//! provider is settling, or paid already, is left to the webhook; the
//! refund stays for the race the sweep cannot cover.
//!
//! The sweep goes shop by shop, a page of carts each, and the next sweep
//! goes on where a page stopped: a shop with many such carts delays its own
//! later carts a few sweeps, never another shop's. A shop that is suspended
//! or took its provider account away is skipped before its carts are read.
//! A payment the sweep left alone is asked about again only after
//! [`RECHECK_SECS`], and the first payment the provider will not answer for
//! (a revoked key, the provider down) ends the shop's sweep until the next.
//!
//! [`webhook`]: super::webhook

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::db::Db;
use anyhow::Context as _;
use timada_discounts::RESERVATION_SECS;
use timada_orders::{CartCursor, OrderError, paying_carts_before, paying_tenants_before};
use tokio::task::JoinHandle;

use super::{Cancellation, DASHBOARD_ACTOR, Payments};
use crate::cluster::Executor;

/// The most carts one sweep looks at per shop; the rest wait for the next
/// sweep, which goes on after them.
const CARTS_PER_TENANT: i64 = 200;

/// How long a payment the sweep left alone (paid, its webhook to come; being
/// settled by the provider) waits before the sweep asks the provider about
/// it again.
pub const RECHECK_SECS: u64 = 600;

/// Unix seconds now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The sweep, with what it needs outside any request.
pub struct LapsedCheckouts {
    /// The read models: the carts with a payment open.
    db: Db,
    /// The store, every tenant's.
    store: Executor,
    payments: Arc<Payments>,
    /// Where the last sweep stopped in each tenant's carts, for the tenants
    /// whose page was full.
    cursors: Mutex<HashMap<String, CartCursor>>,
    /// The payments left alone, by provider id, and the unix seconds from
    /// which the sweep asks about each again.
    recheck_at: Mutex<HashMap<String, u64>>,
}

/// The guard, poisoned or not: nothing held under these locks is left half
/// written by a panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl LapsedCheckouts {
    pub fn new(db: Db, store: Executor, payments: Arc<Payments>) -> Arc<Self> {
        Arc::new(Self {
            db,
            store,
            payments,
            cursors: Mutex::default(),
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
                    tracing::error!(error = %err, "lapsed checkouts not swept");
                }
            }
        })
    }

    /// Cancels the payments open past their hold at `now`, tenant by tenant;
    /// returns how many were. A tenant that fails is logged and the others
    /// still go.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        if !self.payments.enabled() {
            return Ok(0);
        }
        let before = now.saturating_sub(RESERVATION_SECS) as i64;
        let tenants = paying_tenants_before(&self.db, before).await?;
        let mut cancelled = 0;
        for tenant_id in &tenants {
            match self.sweep_tenant(tenant_id, before, now).await {
                Ok(count) => cancelled += count,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, error = %err, "lapsed checkouts not cancelled");
                }
            }
        }
        // A payment whose time to be asked about again came is asked about
        // when its cart comes up; one that went away meanwhile (an order,
        // a refund) is forgotten with it.
        lock(&self.recheck_at).retain(|_, at| *at > now);
        Ok(cancelled)
    }

    /// The read model says which carts of the tenant had a payment open and
    /// were last touched before the hold ran out; the write model says which
    /// payments are still open and when each was opened.
    async fn sweep_tenant(&self, tenant_id: &str, before: i64, now: u64) -> anyhow::Result<usize> {
        let account = match self.payments.active_tenant(tenant_id).await? {
            Some(tenant) => self.payments.account_of(&tenant)?,
            None => None,
        };
        let Some(account) = account else {
            // Suspended, or no provider to cancel with: its carts are not
            // read, and its place is forgotten.
            lock(&self.cursors).remove(tenant_id);
            return Ok(0);
        };
        let after = lock(&self.cursors).get(tenant_id).cloned();
        let carts = paying_carts_before(
            &self.db,
            tenant_id,
            before,
            after.as_ref(),
            CARTS_PER_TENANT,
        )
        .await?;
        let orders = timada_orders::Command::new(self.store.clone(), tenant_id);
        let mut cancelled = 0;
        for row in &carts {
            let Some(cart) = orders.load_cart(&row.id).await? else {
                continue;
            };
            for payment in cart.lapsed_payments(now, RESERVATION_SECS) {
                let payment_id = payment.payment_id.as_str();
                let left_alone = lock(&self.recheck_at)
                    .get(payment_id)
                    .is_some_and(|at| *at > now);
                if left_alone {
                    continue;
                }
                let answer = self
                    .payments
                    .gateway
                    .cancel_payment(&account.secret_key, payment_id)
                    .await
                    .with_context(|| {
                        format!(
                            "cancelling the payment {payment_id} of the cart {}",
                            cart.id
                        )
                    })?;
                match answer {
                    Cancellation::Cancelled => {
                        match orders
                            .cancel_payment(DASHBOARD_ACTOR, &cart.id, payment_id)
                            .await
                        {
                            Ok(()) => {
                                tracing::info!(%tenant_id, cart = %cart.id, payment = %payment_id, "the payment of a lapsed checkout is cancelled");
                                cancelled += 1;
                            }
                            // The cart moved on in between (paid, given
                            // back): nothing to record.
                            Err(OrderError::PaymentNotFound) => {}
                            Err(err) => return Err(err.into()),
                        }
                    }
                    // The webhook settles it; asked about again later if
                    // still open.
                    Cancellation::Confirming | Cancellation::Paid => {
                        tracing::debug!(%tenant_id, cart = %cart.id, payment = %payment_id, ?answer, "a lapsed checkout's payment is left to the shopper");
                        lock(&self.recheck_at).insert(payment_id.to_owned(), now + RECHECK_SECS);
                    }
                }
            }
        }
        // The next sweep goes on after a full page, and from the top after
        // the last one.
        let mut cursors = lock(&self.cursors);
        match carts
            .last()
            .filter(|_| carts.len() as i64 == CARTS_PER_TENANT)
        {
            Some(last) => {
                cursors.insert(tenant_id.to_owned(), CartCursor::from(last));
            }
            None => {
                cursors.remove(tenant_id);
            }
        }
        Ok(cancelled)
    }
}
