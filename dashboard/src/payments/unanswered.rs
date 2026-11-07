//! Answers the requests to pay nobody answered.
//!
//! A request is answered by the node that claims it (`Cart::CheckoutClaimed`);
//! the other nodes move past it. A claimant that stops for good, or that
//! cannot get through (Stripe busy past its retries, writes paused), leaves
//! the request waiting, and the shopper's page with it. So every node sweeps,
//! on a timer, the carts the read model says have a request older than a
//! claim's lease ([`CHECKOUT_CLAIM_SECS`]) without an answer, and refuses
//! each one no claim holds any more (`payments_unavailable`). What its
//! checkout held goes back when that refusal is the answer and the request
//! is still the cart's latest. Two nodes refusing at once write one answer.

use std::sync::Arc;
use std::time::Duration;

use timada_orders::{CHECKOUT_CLAIM_SECS, PAYMENTS_UNAVAILABLE, unanswered_carts};
use tokio::task::JoinHandle;

use super::subscription::refuse_and_give_back;
use crate::cluster::Executor;
use crate::db::Db;

/// The most carts one sweep looks at, every shop together.
const CARTS_PER_SWEEP: i64 = 500;

/// Unix seconds now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The sweep, with what it needs outside any request.
pub struct UnansweredCheckouts {
    /// The read models: the carts with a request unanswered.
    db: Db,
    /// The store, every tenant's.
    store: Executor,
}

impl UnansweredCheckouts {
    pub fn new(db: Db, store: Executor) -> Arc<Self> {
        Arc::new(Self { db, store })
    }

    /// Sweeps every `every`, the first time right away.
    pub fn start(self: Arc<Self>, every: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if let Err(err) = self.sweep(now()).await {
                    tracing::error!(error = %err, "unanswered requests to pay not swept");
                }
            }
        })
    }

    /// Refuses the requests to pay left unanswered at `now` whose claim, if
    /// any, has lapsed; returns how many it refused. A cart that fails is
    /// logged and the others go on.
    pub async fn sweep(&self, now: u64) -> anyhow::Result<usize> {
        let before = now.saturating_sub(CHECKOUT_CLAIM_SECS) as i64;
        let due = unanswered_carts(&self.db, before, CARTS_PER_SWEEP).await?;
        let mut refused = 0;
        for row in &due {
            match self.sweep_cart(&row.tenant_id, &row.id, now).await {
                Ok(count) => refused += count,
                Err(err) => {
                    tracing::error!(tenant = %row.tenant_id, cart = %row.id, error = %err, "a cart's unanswered requests not swept");
                }
            }
        }
        Ok(refused)
    }

    async fn sweep_cart(&self, tenant_id: &str, cart_id: &str, now: u64) -> anyhow::Result<usize> {
        let orders = timada_orders::Command::new(self.store.clone(), tenant_id);
        let Some(cart) = orders.load_cart(cart_id).await? else {
            return Ok(0);
        };
        if cart.is_checked_out() {
            return Ok(0);
        }
        let mut refused = 0;
        for request in cart.requests.iter().filter(|r| r.answer.is_none()) {
            let lease_over = request.requested_at.saturating_add(CHECKOUT_CLAIM_SECS) <= now;
            let claimed = request
                .claim
                .as_ref()
                .is_some_and(|claim| claim.holds_at(now));
            if !lease_over || claimed {
                continue;
            }
            let holds = orders.checkout_holds(cart_id, &request.request_id).await?;
            let answered = refuse_and_give_back(
                &self.store,
                tenant_id,
                cart_id,
                &request.request_id,
                PAYMENTS_UNAVAILABLE,
                holds.as_ref(),
            )
            .await?;
            if answered {
                let claimant = request.claim.as_ref().map(|claim| claim.claimant.as_str());
                tracing::warn!(%tenant_id, %cart_id, request = %request.request_id, ?claimant, "a request to pay nobody answered is refused");
                refused += 1;
            }
        }
        Ok(refused)
    }
}
