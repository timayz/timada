//! Answers the requests to pay of every tenant's carts.
//!
//! The subscription follows the carts and hands each `CheckoutRequested` to
//! this node's [`Checkouts`], which answers it off the subscription's
//! cursor: up to [`LANES`] carts at once, a cart's requests one after the
//! other. A pass that answered inside the handler would answer one request
//! at a time for the whole region, each costing the supplier's read, the
//! pricing loads, the holds and Stripe's round trip: a campaign on one shop
//! would push every shop's shoppers past the storefront's wait (#467).
//! Instead the handler queues the request and acknowledges it, and the
//! lanes answer in parallel; the claim on the cart
//! (`Cart::CheckoutClaimed`, written at the version read) is still what
//! makes one node of the region the one that answers.
//!
//! A fault of the store's (a write it is not taking, a read that failed,
//! Stripe still opening the same payment) no longer fails the pass: the
//! request goes back to the queue for a while ([`RETRY_DELAYS_SECS`]),
//! and the next requests are answered meanwhile. A request the retries
//! cannot answer is left to the sweep of unanswered requests
//! (`unanswered.rs`), which refuses it once the claim's lease is over. A
//! node that stops with requests queued loses them the same way: nobody
//! waits for them by then (the storefront gave up after seconds), and the
//! sweep refuses them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_catalog::CatalogError;
use timada_discounts::{
    DISCOUNT_ALREADY_USED, DISCOUNT_EXHAUSTED, DISCOUNT_UNKNOWN, DiscountError, RESERVATION_SECS,
};
use timada_invoicing::tax::{self, Destination, TaxProfile, TaxRefusal};
use timada_orders::aggregator::CheckoutRequested;
use timada_orders::{
    CheckoutClaim, CheckoutHolds, INVOICING_NOT_CONFIGURED, OSS_REQUIRED_FOR_CURRENCY,
    OpenedPayment, OrderError, PAYMENTS_UNAVAILABLE, PROVIDER_STRIPE,
};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::webhook::OUT_OF_STOCK;
use super::{
    Busy, DASHBOARD_ACTOR, METADATA_CART, METADATA_TENANT, NewPayment, Payments, SuppliedGoods,
};
use crate::ecb::{EuroRates, RateAnswer};
use crate::saga_fault::{dead_letter, event_fault, recorded};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/payments";

/// A request older than this is history being replayed (a node catching up)
/// or one nobody waits for any more: it is refused, never charged.
const ANSWER_WINDOW_SECS: u64 = 10 * 60;

// A node that takes a lapsed claim on finds the request past the window, a
// few minutes of clock difference between the nodes included: it refuses
// it, and never holds or charges what the node that claimed it first may
// still be holding or charging for it.
const _: () = assert!(timada_orders::CHECKOUT_CLAIM_SECS > ANSWER_WINDOW_SECS);

/// How many carts one node answers at once. Within a cart, one request
/// after the other: a later request takes the holds of the one before it,
/// and must find it answered.
pub const LANES: usize = 16;

/// How long a request whose answer met a fault of the store's waits before
/// it is tried again, by attempt; then it is left to the sweep. The last
/// try comes within the answer window of a request answered at once.
pub const RETRY_DELAYS_SECS: [u64; 8] = [1, 2, 5, 15, 30, 60, 120, 300];

const _: () = {
    let mut total = 0;
    let mut i = 0;
    while i < RETRY_DELAYS_SECS.len() {
        total += RETRY_DELAYS_SECS[i];
        i += 1;
    }
    assert!(total < ANSWER_WINDOW_SECS);
};

/// How often the queue is looked at without a lane or the subscription
/// waking the desk: a retry falls due.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// The guard, poisoned or not: nothing held under this lock is left half
/// written by a panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What answers requests to pay on one node.
pub struct Checkouts {
    payments: Arc<Payments>,
    euro_rates: Arc<dyn EuroRates>,
    goods: Arc<dyn SuppliedGoods>,
    /// The store, to write the answers with (a handler only borrows the one
    /// it runs on).
    store: Evento,
    /// This node, as its claims name it.
    node: String,
    /// The requests waiting, and the carts being answered.
    desk: Mutex<Desk>,
    /// Rung by the subscription once it queued a request, and by a lane
    /// once it is done.
    wake: Notify,
}

/// A request to pay waiting for a lane.
#[derive(Debug, Clone)]
struct Request {
    /// The `CheckoutRequested`, for its dead letter should it be given up
    /// on.
    event: evento::Event,
    request_id: String,
    /// Unix seconds: when the shopper asked.
    requested_at: u64,
    /// Unix seconds: not answered before.
    due_at: u64,
    /// How many times a fault of the store's sent it back.
    attempts: usize,
}

impl Request {
    /// The cart, in its tenant.
    fn cart(&self) -> (String, String) {
        (self.event.tenant.clone(), self.event.aggregate_id.clone())
    }
}

/// The queue: the requests in the order they came, and the carts a lane is
/// answering for.
#[derive(Debug, Default)]
struct Desk {
    waiting: VecDeque<Request>,
    answering: HashSet<(String, String)>,
}

impl Desk {
    /// The requests to answer at `now`, moved to the lanes: each due, its
    /// cart neither being answered for nor waiting on an earlier request
    /// of its own, up to the lanes free. The others stay in their order.
    fn ready(&mut self, now: u64) -> Vec<Request> {
        let mut behind = HashSet::new();
        let mut ready = Vec::new();
        let mut index = 0;
        while index < self.waiting.len() && self.answering.len() < LANES {
            let cart = self.waiting[index].cart();
            if self.waiting[index].due_at <= now
                && !self.answering.contains(&cart)
                && !behind.contains(&cart)
            {
                let request = self.waiting.remove(index).expect("in range");
                self.answering.insert(cart);
                ready.push(request);
            } else {
                behind.insert(cart);
                index += 1;
            }
        }
        ready
    }

    /// The lane for `request`'s cart is done with it.
    fn done(&mut self, request: &Request) {
        self.answering.remove(&request.cart());
    }

    /// `request` waits again, ahead of what came after it: a later request
    /// of its cart must still find it answered first.
    fn put_back(&mut self, request: Request) {
        self.waiting.push_front(request);
    }
}

impl Checkouts {
    /// `node` names this node among the others (`node-2`), the same across
    /// its restarts.
    pub fn new(
        payments: Arc<Payments>,
        euro_rates: Arc<dyn EuroRates>,
        goods: Arc<dyn SuppliedGoods>,
        store: Evento,
        node: impl Into<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            payments,
            euro_rates,
            goods,
            store,
            node: node.into(),
            desk: Mutex::default(),
            wake: Notify::new(),
        })
    }

    /// Queues the request to pay `request_id` of a cart (`event` is the
    /// request, `requested_at` unix seconds) for a lane. What the
    /// subscription does with each event.
    pub fn take(&self, event: evento::Event, request_id: &str, requested_at: u64) {
        lock(&self.desk).waiting.push_back(Request {
            event,
            request_id: request_id.to_owned(),
            requested_at,
            due_at: 0,
            attempts: 0,
        });
        self.wake.notify_one();
    }

    /// How many requests wait for a lane or are being answered.
    pub fn queued(&self) -> usize {
        let desk = lock(&self.desk);
        desk.waiting.len() + desk.answering.len()
    }

    /// Answers what is queued, as soon as the subscription queues a request
    /// or a lane is done, and every [`LOOK_EVERY`] for the retries falling
    /// due. The lanes run on their own: a slow supplier or a slow Stripe
    /// holds its cart's lane, never the next look at the queue.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(LOOK_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = self.wake.notified() => {}
                }
                self.dispatch(now());
            }
        })
    }

    /// Starts a lane for each request ready at `now` (unix seconds).
    fn dispatch(self: &Arc<Self>, now: u64) {
        for request in lock(&self.desk).ready(now) {
            let checkouts = self.clone();
            tokio::spawn(async move { checkouts.lane(request, now).await });
        }
    }

    /// One request, answered; a fault of the store's puts it back for
    /// later, until the retries run out.
    async fn lane(self: &Arc<Self>, mut request: Request, now: u64) {
        let (tenant_id, cart_id) = request.cart();
        let answered = self
            .answer(
                &request.event,
                &tenant_id,
                &cart_id,
                &request.request_id,
                request.requested_at,
            )
            .await;
        let mut desk = lock(&self.desk);
        desk.done(&request);
        if let Err(err) = answered {
            match RETRY_DELAYS_SECS.get(request.attempts) {
                Some(delay) => {
                    tracing::warn!(tenant = %tenant_id, %cart_id, request = %request.request_id, attempt = request.attempts + 1, error = %format!("{err:#}"), "a request to pay met a fault of the store's: tried again in {delay} s");
                    request.attempts += 1;
                    request.due_at = now + delay;
                    desk.put_back(request);
                }
                None => {
                    tracing::error!(tenant = %tenant_id, %cart_id, request = %request.request_id, error = %format!("{err:#}"), "a request to pay could not be answered: left to the sweep of unanswered requests");
                }
            }
        }
        drop(desk);
        self.wake.notify_one();
    }
}

/// Follows every tenant's carts and queues each request to pay for this
/// node's lanes. Every node with the tenant open runs it, and the first to
/// claim a request (`Cart::CheckoutClaimed`) is the one that answers it:
/// the others leave it alone, so the goods and the code are held, and the
/// payment opened, once.
pub fn subscription<E: Executor>(checkouts: Arc<Checkouts>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(checkouts)
        .handler(on_checkout_requested())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

#[evento::subscription]
async fn on_checkout_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<CheckoutRequested>,
) -> anyhow::Result<()> {
    let checkouts: Arc<Checkouts> = context.extract();
    checkouts.take((*event).clone(), &event.data.request_id, event.timestamp);
    Ok(())
}

impl Checkouts {
    /// Answers the request to pay `request_id` of a cart (`event` is the
    /// request), asked at `requested_at` (unix seconds), with a payment to
    /// confirm or a refusal, unless another node claimed it first. A fault
    /// of the store's is worth asking again: this node's claim holds
    /// meanwhile, and so does what it held for the checkout. Any other
    /// fault (a cart that cannot be priced, a hold that cannot be taken for
    /// a reason of its own) refuses the request for now
    /// (`payments_unavailable`) and gives back what was held, and the request
    /// is given up on, a dead letter of the subscription that queued it: the
    /// shopper asks again, and the lane goes on to the next cart (#483).
    pub async fn answer(
        &self,
        event: &evento::Event,
        tenant_id: &str,
        cart_id: &str,
        request_id: &str,
        requested_at: u64,
    ) -> anyhow::Result<()> {
        let mut holds = None;
        let answered = self
            .try_answer(tenant_id, cart_id, request_id, requested_at, &mut holds)
            .await;
        let Err(err) = answered else {
            return Ok(());
        };
        if event_fault(&err).is_none() {
            return Err(err);
        }
        // Written first, so a refusal the store fails is tried again with
        // the row already there; writing it again replaces it.
        dead_letter(&self.store, SUBSCRIPTION, event, &err, now()).await?;
        let refused = refuse_and_give_back(
            &self.store,
            tenant_id,
            cart_id,
            request_id,
            PAYMENTS_UNAVAILABLE,
            holds.as_ref(),
        )
        .await
        .map(|_| ());
        recorded(refused, "the refusal of the request to pay")
    }

    /// [`answer`](Self::answer) up to the fault; `holds` is what the
    /// checkout holds once it holds anything, for the caller to give back.
    async fn try_answer(
        &self,
        tenant_id: &str,
        cart_id: &str,
        request_id: &str,
        requested_at: u64,
        holds: &mut Option<CheckoutHolds>,
    ) -> anyhow::Result<()> {
        let payments = &self.payments;
        let orders = timada_orders::Command::new(self.store.clone(), tenant_id);
        match orders
            .claim_checkout(DASHBOARD_ACTOR, cart_id, request_id, &self.node)
            .await?
        {
            CheckoutClaim::Taken => {}
            CheckoutClaim::Held | CheckoutClaim::Answered => return Ok(()),
        }
        let refuse = async |code: &str| {
            orders
                .refuse_payment_request(DASHBOARD_ACTOR, cart_id, request_id, code)
                .await
        };
        let now = now();
        // A request nobody waits for any more (this node replaying its
        // history after a long absence) is refused before anything is read
        // for it: no supplier call, no pricing.
        if now.saturating_sub(requested_at) > ANSWER_WINDOW_SECS {
            refuse(PAYMENTS_UNAVAILABLE).await?;
            return Ok(());
        }

        // What the shop buys as it sells is read first, so the checks below
        // go by what the supplier asks now: a price that moved is
        // `price_changed`, goods it ran out of are `out_of_stock`.
        if let Some(cart) = orders.load_cart(cart_id).await?
            && !cart.is_checked_out()
            && cart
                .request(request_id)
                .is_some_and(|request| request.answer.is_none())
        {
            let products: Vec<String> = cart
                .lines
                .iter()
                .map(|line| line.product_id.clone())
                .collect();
            self.goods.read_again(tenant_id, &products).await;
        }

        let mut pending = match orders.pending_payment(cart_id, request_id).await {
            Ok(Some(pending)) => pending,
            Ok(None) => return Ok(()),
            Err(err) => match err.code() {
                Some(code) => {
                    refuse(code).await?;
                    return Ok(());
                }
                None => return Err(err.into()),
            },
        };
        // Goods a supplier buys once the order is placed keep the money held
        // until they are bought; otherwise it is taken as soon as the order is.
        pending.supplier = self.goods.supplier_of(tenant_id, &pending.lines).await?;
        let Some(merchant) = payments.active_tenant(&self.store, tenant_id).await? else {
            refuse(PAYMENTS_UNAVAILABLE).await?;
            return Ok(());
        };
        // No sale without an invoice that can name the merchant.
        let Some(legal) = payments
            .company_of(&self.store, &merchant)
            .await?
            .and_then(|company| company.identity)
        else {
            refuse(INVOICING_NOT_CONFIGURED).await?;
            return Ok(());
        };
        // Nor one whose VAT the merchant could not remit: an EU distance sale
        // in another currency than the euro counts against the threshold at the
        // European Central Bank's euro rate, so a currency the Bank publishes
        // no rate for needs the One-Stop Shop. The buyer's VAT number is taken
        // at its word here; invoicing checks it with VIES.
        let profile = TaxProfile {
            vat_registered: legal.is_vat_registered(),
            oss_registered: legal.oss_registered,
        };
        let destination = Destination {
            country: pending.request.shipping.country.clone(),
            postal_code: pending.request.shipping.postal_code.clone(),
            vat_number: pending.request.vat_number.clone(),
        };
        if tax::needs_euro_rate(profile, &destination, &pending.currency) {
            let day = timada_core::civil::format_date(now);
            let published = match self.euro_rates.rate_on(&pending.currency, &day).await {
                RateAnswer::Rate(_) => true,
                RateAnswer::NoSuchCurrency => false,
                // Not a refusal of the sale: the request is answered once the
                // rates can be read.
                RateAnswer::Unavailable(reason) => {
                    tracing::warn!(tenant = %tenant_id, %cart_id, currency = %pending.currency, %reason, "the euro reference rates could not be read: payment refused for now");
                    refuse(PAYMENTS_UNAVAILABLE).await?;
                    return Ok(());
                }
            };
            if let Err(TaxRefusal::OssRequiredForCurrency) =
                tax::accepts(profile, &destination, &pending.currency, published)
            {
                refuse(OSS_REQUIRED_FOR_CURRENCY).await?;
                return Ok(());
            }
        }
        // A shop that stopped taking payments opens none, its keys kept only
        // to settle what is held or owed.
        let Some(account) = payments.checkout_account_of(&merchant)? else {
            refuse(PAYMENTS_UNAVAILABLE).await?;
            return Ok(());
        };
        // The discount's use is held before the shopper is asked to pay, so a
        // capped code never serves more orders than it says. A refusal below,
        // once it is the answer, gives it back.
        let discounts = timada_discounts::Command::new(self.store.clone(), tenant_id);
        let email = pending.request.email.clone();
        let discount_id = pending.discount.as_ref().map(|d| d.discount_id.clone());
        if let Some(discount_id) = &discount_id {
            let reserved = discounts
                .reserve_use(
                    DASHBOARD_ACTOR,
                    discount_id,
                    cart_id,
                    &email,
                    RESERVATION_SECS,
                )
                .await;
            match reserved {
                Ok(()) => {}
                Err(DiscountError::Exhausted) => {
                    refuse(DISCOUNT_EXHAUSTED).await?;
                    return Ok(());
                }
                Err(DiscountError::AlreadyUsed) => {
                    refuse(DISCOUNT_ALREADY_USED).await?;
                    return Ok(());
                }
                // A store not taking writes for the moment, or not this one,
                // is not a refusal of the code: the error retries the payment
                // request later.
                Err(err)
                    if err.is_user_facing()
                        && !err.code().is_some_and(timada_core::is_write_refusal) =>
                {
                    refuse(DISCOUNT_UNKNOWN).await?;
                    return Ok(());
                }
                Err(err) => return Err(err.into()),
            }
        }
        // The goods are held the same way, so two shoppers never pay for the
        // last one. From here on, a fault gives back what is held.
        let holds = holds.insert(CheckoutHolds::from(&pending));
        let catalog = timada_catalog::Command::new(self.store.clone(), tenant_id);
        match catalog
            .reserve_stock(
                DASHBOARD_ACTOR,
                cart_id,
                &holds.stock_lines,
                RESERVATION_SECS,
            )
            .await
        {
            Ok(()) => {}
            // What it held of the goods went back with the refusal of the
            // last thing: the discount's use is left.
            Err(CatalogError::OutOfStock { .. }) => {
                let discount_only = CheckoutHolds {
                    stock_lines: Vec::new(),
                    ..holds.clone()
                };
                refuse_and_give_back(
                    &self.store,
                    tenant_id,
                    cart_id,
                    request_id,
                    OUT_OF_STOCK,
                    Some(&discount_only),
                )
                .await?;
                return Ok(());
            }
            Err(err) => return Err(err.into()),
        }
        let opened = payments
            .gateway
            .open_payment(
                &account.secret_key,
                NewPayment {
                    amount_cents: pending.total_cents,
                    currency: pending.currency.clone(),
                    idempotency_key: format!("timada-{request_id}"),
                    metadata: HashMap::from([
                        (METADATA_TENANT.to_owned(), tenant_id.to_owned()),
                        (METADATA_CART.to_owned(), cart_id.to_owned()),
                        ("request_id".to_owned(), request_id.to_owned()),
                    ]),
                    receipt_email: Some(pending.request.email.clone()),
                    description: None,
                },
            )
            .await;
        let opened = match opened {
            Ok(opened) => opened,
            // Stripe is still answering a request under the same key: asked
            // again in a moment, it answers with that payment.
            Err(err) if err.downcast_ref::<Busy>().is_some() => return Err(err),
            Err(err) => {
                tracing::warn!(tenant = %tenant_id, %cart_id, %err, "Stripe refused to open a payment");
                refuse_and_give_back(
                    &self.store,
                    tenant_id,
                    cart_id,
                    request_id,
                    PAYMENTS_UNAVAILABLE,
                    Some(holds),
                )
                .await?;
                return Ok(());
            }
        };
        let recorded = orders
            .record_payment_request(
                DASHBOARD_ACTOR,
                cart_id,
                pending,
                OpenedPayment {
                    provider: PROVIDER_STRIPE.to_owned(),
                    payment_id: opened.payment_id,
                    client_secret: opened.client_secret,
                    publishable_key: account.publishable_key,
                },
            )
            .await;
        match recorded {
            Ok(()) => Ok(()),
            // Retried: the payment opens again under the same idempotency key
            // and the reservation is held again for the same cart.
            Err(err @ (OrderError::Write(_) | OrderError::Other(_))) => Err(err.into()),
            Err(err) => {
                give_back(&self.store, tenant_id, cart_id, holds).await;
                tracing::warn!(%cart_id, %err, "a payment opened for a cart that cannot record it");
                Ok(())
            }
        }
    }
}

/// Unix seconds now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Refuses the request to pay with `code`. What its checkout holds
/// (`holds`) goes back only when this refusal is the request's answer (a
/// payment recorded meanwhile keeps it) and the request is still the cart's
/// latest (a later one took the cart's holds on for itself). Whether this
/// refusal answered it.
pub(super) async fn refuse_and_give_back(
    store: &Evento,
    tenant_id: &str,
    cart_id: &str,
    request_id: &str,
    code: &str,
    holds: Option<&CheckoutHolds>,
) -> anyhow::Result<bool> {
    let orders = timada_orders::Command::new(store.clone(), tenant_id);
    if !orders
        .refuse_payment_request(DASHBOARD_ACTOR, cart_id, request_id, code)
        .await?
    {
        return Ok(false);
    }
    let latest = orders
        .load_cart(cart_id)
        .await?
        .is_some_and(|cart| cart.is_latest_request(request_id));
    if latest && let Some(holds) = holds {
        give_back(store, tenant_id, cart_id, holds).await;
    }
    Ok(true)
}

/// Gives back what the checkout of `cart_id` holds: the goods, the
/// discount's use. A failure is only logged: a hold lapses on its own.
pub(super) async fn give_back(
    store: &Evento,
    tenant_id: &str,
    cart_id: &str,
    holds: &CheckoutHolds,
) {
    if let Some(discount_id) = &holds.discount_id
        && let Err(err) = timada_discounts::Command::new(store.clone(), tenant_id)
            .release_use(DASHBOARD_ACTOR, discount_id, cart_id, &holds.email)
            .await
    {
        tracing::warn!(%cart_id, %err, "cannot give a discount's use back");
    }
    if !holds.stock_lines.is_empty()
        && let Err(err) = timada_catalog::Command::new(store.clone(), tenant_id)
            .release_stock(DASHBOARD_ACTOR, cart_id, &holds.stock_lines)
            .await
    {
        tracing::warn!(%cart_id, %err, "cannot give back the stock a checkout held");
    }
}

#[cfg(test)]
mod desk_tests {
    use super::*;

    fn request(tenant: &str, cart: &str, id: &str, due_at: u64) -> Request {
        Request {
            event: evento::Event {
                tenant: tenant.to_owned(),
                aggregate_id: cart.to_owned(),
                name: "CheckoutRequested".into(),
                ..Default::default()
            },
            request_id: id.to_owned(),
            requested_at: 0,
            due_at,
            attempts: 0,
        }
    }

    fn ids(requests: &[Request]) -> Vec<&str> {
        requests.iter().map(|r| r.request_id.as_str()).collect()
    }

    #[test]
    fn carts_go_at_once_and_a_carts_requests_one_after_the_other() {
        let mut desk = Desk::default();
        desk.waiting.push_back(request("t", "c1", "r1", 0));
        desk.waiting.push_back(request("t", "c1", "r2", 0));
        desk.waiting.push_back(request("t", "c2", "r3", 0));
        // The same cart id in another tenant is another cart.
        desk.waiting.push_back(request("u", "c1", "r4", 0));

        let ready = desk.ready(10);
        assert_eq!(ids(&ready), ["r1", "r3", "r4"]);
        assert_eq!(ids(desk.waiting.make_contiguous()), ["r2"]);
        // Nothing for c1 while r1 is being answered.
        assert!(desk.ready(10).is_empty());
        desk.done(&ready[0]);
        assert_eq!(ids(&desk.ready(10)), ["r2"]);
    }

    #[test]
    fn a_request_not_due_holds_its_carts_later_ones_and_no_other() {
        let mut desk = Desk::default();
        desk.waiting.push_back(request("t", "c1", "r1", 50));
        desk.waiting.push_back(request("t", "c1", "r2", 0));
        desk.waiting.push_back(request("t", "c2", "r3", 0));
        assert_eq!(ids(&desk.ready(10)), ["r3"]);
        assert_eq!(ids(desk.waiting.make_contiguous()), ["r1", "r2"]);
        desk.done(&request("t", "c2", "r3", 0));
        assert_eq!(ids(&desk.ready(50)), ["r1"]);
    }

    #[test]
    fn no_more_than_the_lanes_at_once() {
        let mut desk = Desk::default();
        for i in 0..LANES + 3 {
            desk.waiting
                .push_back(request("t", &format!("c{i}"), &format!("r{i}"), 0));
        }
        let ready = desk.ready(0);
        assert_eq!(ready.len(), LANES);
        assert_eq!(desk.waiting.len(), 3);
        assert!(desk.ready(0).is_empty());
        desk.done(&ready[0]);
        assert_eq!(ids(&desk.ready(0)), [format!("r{LANES}").as_str()]);
    }

    #[test]
    fn a_request_put_back_goes_before_what_came_after_it() {
        let mut desk = Desk::default();
        desk.waiting.push_back(request("t", "c1", "r1", 0));
        desk.waiting.push_back(request("t", "c1", "r2", 0));
        let mut first = desk.ready(0).remove(0);
        desk.done(&first);
        first.due_at = 5;
        desk.put_back(first);
        assert!(desk.ready(0).is_empty(), "r2 waits for r1");
        assert_eq!(ids(&desk.ready(5)), ["r1"]);
    }
}
