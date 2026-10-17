//! Answers the requests to pay of one tenant's carts.

use std::collections::HashMap;
use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_discounts::{
    DISCOUNT_ALREADY_USED, DISCOUNT_EXHAUSTED, DISCOUNT_UNKNOWN, DiscountError, RESERVATION_SECS,
};
use timada_orders::aggregator::CheckoutRequested;
use timada_orders::{
    INVOICING_NOT_CONFIGURED, OpenedPayment, OrdersError, PAYMENTS_UNAVAILABLE, PROVIDER_STRIPE,
};

use super::{DASHBOARD_ACTOR, NewPayment, Payments};

/// The cursor identity of the subscription, in each tenant's store.
pub const SUBSCRIPTION: &str = "timada-dashboard/payments";

/// A request older than this is history being replayed (a node catching up)
/// or one nobody waits for any more: it is refused, never charged.
const ANSWER_WINDOW_SECS: u64 = 10 * 60;

#[derive(Clone)]
struct Tenant(String);

/// The tenant's own executor, to write the answers with (a handler only
/// borrows the one it runs on).
#[derive(Clone)]
struct Store(Evento);

/// Follows one tenant's carts and answers each request to pay. Every node
/// with the tenant open runs it: Stripe opens one payment per request (the
/// request id is the idempotency key) and the first answer recorded wins.
pub fn subscription<E: Executor>(
    payments: Arc<Payments>,
    tenant_id: &str,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(payments)
        .data(Store(store))
        .data(Tenant(tenant_id.to_owned()))
        .handler(on_checkout_requested())
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_checkout_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<CheckoutRequested>,
) -> anyhow::Result<()> {
    let payments: Arc<Payments> = context.extract();
    let tenant: Tenant = context.extract();
    let store: Store = context.extract();
    let orders = timada_orders::Command(store.0.clone());
    let cart_id = event.aggregate_id.clone();
    let request_id = event.data.request_id.clone();
    let refuse = async |code: &str| {
        orders
            .refuse_payment_request(DASHBOARD_ACTOR, &cart_id, &request_id, code)
            .await
    };

    let pending = match orders.pending_payment(&cart_id, &request_id).await {
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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if now.saturating_sub(event.timestamp) > ANSWER_WINDOW_SECS {
        refuse(PAYMENTS_UNAVAILABLE).await?;
        return Ok(());
    }
    let Some(merchant) = payments.active_tenant(&tenant.0).await? else {
        refuse(PAYMENTS_UNAVAILABLE).await?;
        return Ok(());
    };
    // No sale without an invoice that can name the merchant.
    if payments.company_of(&merchant).await?.is_none() {
        refuse(INVOICING_NOT_CONFIGURED).await?;
        return Ok(());
    }
    let Some(account) = payments.account_of(&merchant)? else {
        refuse(PAYMENTS_UNAVAILABLE).await?;
        return Ok(());
    };
    // The discount's use is held before the shopper is asked to pay, so a
    // capped code never serves more orders than it says. Every way out below
    // that leaves no payment open gives it back.
    let discounts = timada_discounts::Command(store.0.clone());
    let email = pending.request.email.clone();
    let discount_id = pending.discount.as_ref().map(|d| d.discount_id.clone());
    if let Some(discount_id) = &discount_id {
        let reserved = discounts
            .reserve_use(
                DASHBOARD_ACTOR,
                discount_id,
                &cart_id,
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
            Err(err) if err.is_user_facing() && err.code() != Some(timada_core::WRITES_PAUSED) => {
                refuse(DISCOUNT_UNKNOWN).await?;
                return Ok(());
            }
            Err(err) => return Err(err.into()),
        }
    }
    let release = async || {
        if let Some(discount_id) = &discount_id
            && let Err(err) = discounts
                .release_use(DASHBOARD_ACTOR, discount_id, &cart_id, &email)
                .await
        {
            tracing::warn!(%cart_id, %err, "cannot give a discount's use back");
        }
    };
    let opened = payments
        .gateway
        .open_payment(
            &account.secret_key,
            NewPayment {
                amount_cents: pending.total_cents,
                currency: pending.currency.clone(),
                idempotency_key: format!("timada-{request_id}"),
                metadata: HashMap::from([
                    ("tenant_id".to_owned(), tenant.0.clone()),
                    ("cart_id".to_owned(), cart_id.clone()),
                    ("request_id".to_owned(), request_id.clone()),
                ]),
                receipt_email: Some(pending.request.email.clone()),
                description: None,
            },
        )
        .await;
    let opened = match opened {
        Ok(opened) => opened,
        Err(err) => {
            tracing::warn!(tenant = %tenant.0, %cart_id, %err, "Stripe refused to open a payment");
            release().await;
            refuse(PAYMENTS_UNAVAILABLE).await?;
            return Ok(());
        }
    };
    let recorded = orders
        .record_payment_request(
            DASHBOARD_ACTOR,
            &cart_id,
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
        Err(err @ (OrdersError::Write(_) | OrdersError::Other(_))) => Err(err.into()),
        Err(err) => {
            release().await;
            tracing::warn!(%cart_id, %err, "a payment opened for a cart that cannot record it");
            Ok(())
        }
    }
}
