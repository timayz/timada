//! What a Stripe webhook does once it reached a tenant: check the signature,
//! ask Stripe about the payment, and turn the cart it paid for into the
//! order. Kept apart from the HTTP route so it is tested without one.

use timada_orders::{OrdersError, PROVIDER_STRIPE, Payment};

use super::{Account, DASHBOARD_ACTOR, Payments, signature};
use crate::cluster::Executor;

/// What the route answers Stripe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The order exists (now, or already).
    Placed { order_id: String },
    /// Taken for nothing to sell (a second payment of a cart, an amount that
    /// does not match): given back.
    Refunded { refund_id: String },
    /// Not ours to act on: another event type, a payment not made through
    /// timada, one that did not succeed.
    Ignored,
    /// The signature does not check out, or the body is not an event.
    Rejected,
    /// Not now: Stripe should try again.
    Retry,
}

/// The part of an event read before asking Stripe.
#[derive(serde::Deserialize)]
struct EventHead {
    #[serde(rename = "type")]
    type_: String,
    data: EventData,
}

#[derive(serde::Deserialize)]
struct EventData {
    object: ObjectHead,
}

#[derive(serde::Deserialize)]
struct ObjectHead {
    id: String,
}

/// Handles one webhook of `tenant_id`, whose Stripe account is `account` and
/// whose store `store` is.
pub async fn receive(
    payments: &Payments,
    account: &Account,
    tenant_id: &str,
    store: Executor,
    body: &[u8],
    signature_header: Option<&str>,
    now: i64,
) -> anyhow::Result<Outcome> {
    let signed = signature_header
        .is_some_and(|header| signature::verify(body, header, &account.webhook_secret, now));
    if !signed {
        return Ok(Outcome::Rejected);
    }
    let Ok(event) = serde_json::from_slice::<EventHead>(body) else {
        return Ok(Outcome::Rejected);
    };
    if event.type_ != "payment_intent.succeeded" {
        return Ok(Outcome::Ignored);
    }

    // Stripe's word, not the payload's: the payment as it stands now.
    let status = payments
        .gateway
        .payment_status(&account.secret_key, &event.data.object.id)
        .await?;
    let ours = status.metadata.get("tenant_id").map(String::as_str) == Some(tenant_id);
    let Some(cart_id) = status.metadata.get("cart_id").filter(|_| ours) else {
        return Ok(Outcome::Ignored);
    };
    if !status.succeeded {
        return Ok(Outcome::Ignored);
    }
    let orders = timada_orders::Command::new(store, tenant_id);
    let payment = Payment {
        provider: PROVIDER_STRIPE.to_owned(),
        payment_id: status.payment_id.clone(),
        amount_cents: status.amount_received,
        currency: status.currency.to_ascii_uppercase(),
    };
    match orders
        .place_paid_order(DASHBOARD_ACTOR, cart_id, payment)
        .await
    {
        Ok(placed) => Ok(Outcome::Placed {
            order_id: placed.order_id,
        }),
        Err(err @ (OrdersError::DuplicatePayment { .. } | OrdersError::AmountMismatch)) => {
            tracing::warn!(%tenant_id, payment = %status.payment_id, %err, "refunding a payment");
            let refund_id = payments
                .gateway
                .refund(
                    &account.secret_key,
                    &status.payment_id,
                    status.amount_received,
                    &format!("timada-unplaced-{}", status.payment_id),
                )
                .await?;
            Ok(Outcome::Refunded { refund_id })
        }
        Err(err) => {
            tracing::error!(%tenant_id, payment = %status.payment_id, %err, "a paid cart could not become an order");
            Ok(Outcome::Retry)
        }
    }
}
