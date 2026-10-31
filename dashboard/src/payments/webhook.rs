//! What a Stripe webhook does once it reached a tenant: check the signature,
//! ask Stripe about the payment, and turn the cart it paid for into the
//! order. Kept apart from the HTTP route so it is tested without one.
//!
//! The goods and the discount's use were held when the payment was opened,
//! for [`RESERVATION_SECS`]; a payment confirmed after that (a card form
//! left open, a bank's slow 3-D Secure, a webhook delivered late) finds
//! them lapsed, and someone else may have taken them. So before the order
//! is placed the holds are taken again, which does nothing while they
//! stand and takes what is left otherwise; when the last of it is gone the
//! money goes back ([`Outcome::Returned`]) and the cart records it
//! (`Cart::PaymentReturned`): an order is never placed for goods already
//! sold or a code already spent.
//!
//! A `charge.refunded` is reconciled: the refunds Stripe holds for the
//! payment (made here, or in Stripe's own dashboard) that the order does
//! not know yet are recorded, so a refund made by hand, or one whose
//! recording failed after Stripe took it, reaches the order and its credit
//! note.

use timada_catalog::CatalogError;
use timada_discounts::{
    DISCOUNT_ALREADY_USED, DISCOUNT_EXHAUSTED, DiscountError, RESERVATION_SECS,
};
use timada_orders::{CartState, OrdersError, PROVIDER_STRIPE, Payment, ReturnedPayment};

use super::{Account, DASHBOARD_ACTOR, Payments, signature};
use crate::cluster::Executor;

/// The refusal of a checkout that wants more than is left.
pub const OUT_OF_STOCK: &str = "out_of_stock";

/// What the route answers Stripe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The order exists (now, or already).
    Placed { order_id: String },
    /// Taken for nothing to sell (a second payment of a cart, an amount that
    /// does not match): given back.
    Refunded { refund_id: String },
    /// Paid too late: what the checkout held had lapsed and gone to someone
    /// else, so the money went back and no order was placed (now, or
    /// already). `code` says what was gone.
    Returned { refund_id: String, code: String },
    /// A `charge.refunded` matched against what the order knows: that many
    /// refunds Stripe holds were not recorded yet and are now.
    Reconciled { recorded: usize },
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
    /// On a charge: the payment it belongs to.
    #[serde(default)]
    payment_intent: Option<String>,
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
    let payment_id = match event.type_.as_str() {
        "payment_intent.succeeded" => event.data.object.id.clone(),
        "charge.refunded" => match event.data.object.payment_intent.clone() {
            Some(payment_id) => payment_id,
            None => return Ok(Outcome::Ignored),
        },
        _ => return Ok(Outcome::Ignored),
    };

    // Stripe's word, not the payload's: the payment as it stands now.
    let status = payments
        .gateway
        .payment_status(&account.secret_key, &payment_id)
        .await?;
    let ours = status.metadata.get("tenant_id").map(String::as_str) == Some(tenant_id);
    let Some(cart_id) = status.metadata.get("cart_id").filter(|_| ours) else {
        return Ok(Outcome::Ignored);
    };
    if event.type_ == "charge.refunded" {
        return reconcile_refunds(payments, account, tenant_id, store, cart_id, &payment_id).await;
    }
    if !status.succeeded {
        return Ok(Outcome::Ignored);
    }
    let orders = timada_orders::Command::new(store.clone(), tenant_id);
    let payment = Payment {
        provider: PROVIDER_STRIPE.to_owned(),
        payment_id: status.payment_id.clone(),
        amount_cents: status.amount_received,
        currency: status.currency.to_ascii_uppercase(),
    };
    // Given back already: the same answer, no second refund.
    let cart = orders.load_cart(cart_id).await?;
    if let Some(returned) = cart
        .as_ref()
        .and_then(|cart| cart.returned_payment(&payment.payment_id))
    {
        return Ok(Outcome::Returned {
            refund_id: returned.refund_id.clone(),
            code: returned.code.clone(),
        });
    }
    // Not an order yet: the holds its checkout took may have lapsed since,
    // so they are taken again before the order is placed.
    if let Some(cart) = cart.as_ref().filter(|cart| !cart.is_checked_out())
        && let Some(gone) = retake_holds(&store, tenant_id, cart, &payment.payment_id).await?
    {
        tracing::warn!(%tenant_id, payment = %payment.payment_id, code = %gone, "a late payment finds what it held gone: given back");
        let refund_id = payments
            .gateway
            .refund(
                &account.secret_key,
                &payment.payment_id,
                payment.amount_cents,
                &format!("timada-lapsed-{}", payment.payment_id),
            )
            .await?;
        orders
            .return_payment(
                DASHBOARD_ACTOR,
                cart_id,
                ReturnedPayment {
                    payment_id: payment.payment_id.clone(),
                    refund_id: refund_id.clone(),
                    amount_cents: payment.amount_cents,
                    code: gone.to_owned(),
                },
            )
            .await?;
        return Ok(Outcome::Returned {
            refund_id,
            code: gone.to_owned(),
        });
    }
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

/// Records the refunds Stripe holds for `payment_id` that the order made
/// from `cart_id` does not know yet. No order (the payment never became
/// one) is nothing to reconcile.
async fn reconcile_refunds(
    payments: &Payments,
    account: &Account,
    tenant_id: &str,
    store: Executor,
    cart_id: &str,
    payment_id: &str,
) -> anyhow::Result<Outcome> {
    let orders = timada_orders::Command::new(store, tenant_id);
    let Some(order) = orders.load_order_of_cart(cart_id).await? else {
        return Ok(Outcome::Ignored);
    };
    if order.payment.payment_id != payment_id {
        return Ok(Outcome::Ignored);
    }
    let mut recorded = 0;
    for refund in payments
        .gateway
        .refunds_of(&account.secret_key, payment_id)
        .await?
    {
        if !refund.succeeded || order.refund_ids.contains(&refund.refund_id) {
            continue;
        }
        orders
            .record_refund(
                DASHBOARD_ACTOR,
                &order.id,
                &refund.refund_id,
                refund.amount_cents,
            )
            .await?;
        recorded += 1;
    }
    if recorded > 0 {
        tracing::info!(%tenant_id, order = %order.id, recorded, "refunds made in Stripe recorded on the order");
    }
    Ok(Outcome::Reconciled { recorded })
}

/// Takes again what the checkout paid by `payment_id` held: nothing changes
/// while the holds stand, what is left is taken otherwise. `Some(code)`
/// when the last of it is gone: the cart then holds nothing any more (what
/// it still held, or took here, is given back). `None` when the order can
/// be placed. A cart that opened no such payment changes nothing:
/// `place_paid_order` says what is wrong with it.
async fn retake_holds(
    store: &Executor,
    tenant_id: &str,
    cart: &CartState,
    payment_id: &str,
) -> anyhow::Result<Option<&'static str>> {
    let Some(request) = cart.request_paid_by(payment_id) else {
        return Ok(None);
    };
    let Some(snapshot) = request.payment() else {
        return Ok(None);
    };
    let discounts = timada_discounts::Command::new(store.clone(), tenant_id);
    let catalog = timada_catalog::Command::new(store.clone(), tenant_id);
    let discount_id = snapshot.discount.as_ref().map(|d| d.discount_id.as_str());
    let lines = timada_orders::stock_lines(&snapshot.lines);

    let mut gone = None;
    if let Some(discount_id) = discount_id {
        let held = discounts
            .reserve_use(
                DASHBOARD_ACTOR,
                discount_id,
                &cart.id,
                &request.email,
                RESERVATION_SECS,
            )
            .await;
        match held {
            Ok(()) => {}
            Err(DiscountError::Exhausted) => gone = Some(DISCOUNT_EXHAUSTED),
            Err(DiscountError::AlreadyUsed) => gone = Some(DISCOUNT_ALREADY_USED),
            // The code is gone from the shop: nothing left to hold, the
            // order keeps the price it was paid at.
            Err(DiscountError::NotFound) => {}
            Err(err) => return Err(err.into()),
        }
    }
    if gone.is_none() {
        match catalog
            .reserve_stock(DASHBOARD_ACTOR, &cart.id, &lines, RESERVATION_SECS)
            .await
        {
            Ok(()) => return Ok(None),
            Err(CatalogError::OutOfStock { .. }) => gone = Some(OUT_OF_STOCK),
            Err(err) => return Err(err.into()),
        }
    }
    // Given back: the cart keeps no hold, on the goods or on the code.
    if let Err(err) = catalog
        .release_stock(DASHBOARD_ACTOR, &cart.id, &lines)
        .await
    {
        tracing::warn!(cart = %cart.id, %err, "cannot give back the stock a checkout held");
    }
    if let Some(discount_id) = discount_id
        && let Err(err) = discounts
            .release_use(DASHBOARD_ACTOR, discount_id, &cart.id, &request.email)
            .await
    {
        tracing::warn!(cart = %cart.id, %err, "cannot give a discount's use back");
    }
    Ok(gone)
}
