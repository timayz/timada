//! Answers the requests to pay of every tenant's carts.

use std::collections::HashMap;
use std::sync::Arc;

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

use super::webhook::OUT_OF_STOCK;
use super::{
    Busy, DASHBOARD_ACTOR, METADATA_CART, METADATA_TENANT, NewPayment, Payments, SuppliedGoods,
};
use crate::ecb::{EuroRates, RateAnswer};

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

/// What answers requests to pay on one node.
#[derive(Clone)]
pub struct Checkouts {
    payments: Arc<Payments>,
    euro_rates: Arc<dyn EuroRates>,
    goods: Arc<dyn SuppliedGoods>,
    /// The store, to write the answers with (a handler only borrows the one
    /// it runs on).
    store: Evento,
    /// This node, as its claims name it.
    node: String,
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
    ) -> Self {
        Self {
            payments,
            euro_rates,
            goods,
            store,
            node: node.into(),
        }
    }
}

/// Follows every tenant's carts and answers each request to pay. Every node
/// with the tenant open runs it, and the first to claim a request
/// (`Cart::CheckoutClaimed`) is the one that answers it: the others leave it
/// alone, so the goods and the code are held, and the payment opened, once.
pub fn subscription<E: Executor>(checkouts: Checkouts) -> SubscriptionBuilder<E> {
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
    let checkouts: Checkouts = context.extract();
    checkouts
        .answer(
            &event.tenant,
            &event.aggregate_id,
            &event.data.request_id,
            event.timestamp,
        )
        .await
}

impl Checkouts {
    /// Answers the request to pay `request_id` of a cart, asked at
    /// `requested_at` (unix seconds), with a payment to confirm or a
    /// refusal, unless another node claimed it first. An error is worth
    /// asking again: this node's claim holds meanwhile, and so does what it
    /// held for the checkout.
    pub async fn answer(
        &self,
        tenant_id: &str,
        cart_id: &str,
        request_id: &str,
        requested_at: u64,
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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
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
        let Some(merchant) = payments.active_tenant(tenant_id).await? else {
            refuse(PAYMENTS_UNAVAILABLE).await?;
            return Ok(());
        };
        // No sale without an invoice that can name the merchant.
        let Some(legal) = payments
            .company_of(&merchant)
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
                // A store not taking writes for the moment is not a refusal of
                // the code: the error retries the payment request later.
                Err(err)
                    if err.is_user_facing() && err.code() != Some(timada_core::WRITES_PAUSED) =>
                {
                    refuse(DISCOUNT_UNKNOWN).await?;
                    return Ok(());
                }
                Err(err) => return Err(err.into()),
            }
        }
        // The goods are held the same way, so two shoppers never pay for the
        // last one.
        let holds = CheckoutHolds::from(&pending);
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
                    ..holds
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
                    Some(&holds),
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
                give_back(&self.store, tenant_id, cart_id, &holds).await;
                tracing::warn!(%cart_id, %err, "a payment opened for a cart that cannot record it");
                Ok(())
            }
        }
    }
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
