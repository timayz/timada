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
//! shopper is confirming at that moment, or paid already, is left to the
//! webhook; the refund stays for the race the sweep cannot cover.
//!
//! [`webhook`]: super::webhook

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use timada_discounts::RESERVATION_SECS;
use timada_orders::{CartRow, OrderError, paying_carts_before};
use tokio::task::JoinHandle;

use super::{Cancellation, DASHBOARD_ACTOR, Payments};
use crate::cluster::Executor;

/// The most carts one sweep looks at, over every shop; the rest wait for the
/// next.
const CARTS_PER_SWEEP: i64 = 500;

/// Unix seconds now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The sweep, with what it needs outside any request.
pub struct LapsedCheckouts {
    read_pool: SqlitePool,
    /// The store, every tenant's.
    store: Executor,
    payments: Arc<Payments>,
}

impl LapsedCheckouts {
    pub fn new(read_pool: SqlitePool, store: Executor, payments: Arc<Payments>) -> Arc<Self> {
        Arc::new(Self {
            read_pool,
            store,
            payments,
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
        let carts = paying_carts_before(&self.read_pool, before, CARTS_PER_SWEEP).await?;
        let mut cancelled = 0;
        for tenant in carts.chunk_by(|a, b| a.tenant_id == b.tenant_id) {
            let tenant_id = &tenant[0].tenant_id;
            match self.sweep_tenant(tenant_id, tenant, now).await {
                Ok(count) => cancelled += count,
                Err(err) => {
                    tracing::error!(tenant = %tenant_id, error = %err, "lapsed checkouts not cancelled");
                }
            }
        }
        Ok(cancelled)
    }

    /// The read model said these carts of the tenant had a payment open and
    /// were last touched before the hold ran out; the write model says which
    /// payments are still open and when each was opened.
    async fn sweep_tenant(
        &self,
        tenant_id: &str,
        carts: &[CartRow],
        now: u64,
    ) -> anyhow::Result<usize> {
        let Some(tenant) = self.payments.active_tenant(tenant_id).await? else {
            return Ok(0);
        };
        let Some(account) = self.payments.account_of(&tenant)? else {
            return Ok(0);
        };
        let orders = timada_orders::Command::new(self.store.clone(), tenant_id);
        let mut cancelled = 0;
        for row in carts {
            let Some(cart) = orders.load_cart(&row.id).await? else {
                continue;
            };
            for payment in cart.lapsed_payments(now, RESERVATION_SECS) {
                let payment_id = payment.payment_id.as_str();
                let answer = self
                    .payments
                    .gateway
                    .cancel_payment(&account.secret_key, payment_id)
                    .await;
                match answer {
                    Ok(Cancellation::Cancelled) => {
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
                    // The webhook settles it; the next sweep looks again
                    // at one still open.
                    Ok(Cancellation::Confirming | Cancellation::Paid) => {
                        tracing::debug!(%tenant_id, cart = %cart.id, payment = %payment_id, answer = ?answer, "a lapsed checkout's payment is left to the shopper");
                    }
                    Err(err) => {
                        tracing::warn!(%tenant_id, cart = %cart.id, payment = %payment_id, %err, "cannot cancel the payment of a lapsed checkout");
                    }
                }
            }
        }
        Ok(cancelled)
    }
}
