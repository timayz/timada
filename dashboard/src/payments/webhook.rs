//! What a Stripe webhook does once it reached a tenant: check the signature,
//! ask Stripe about the payment, and bring the cart and its order in step
//! with what Stripe says. Kept apart from the HTTP route so it is tested
//! without one.
//!
//! The shopper confirming the payment holds the money on their card
//! (`payment_intent.amount_capturable_updated`), and the cart becomes the
//! order. The goods and the discount's use were held when the payment was
//! opened, for [`RESERVATION_SECS`]; a payment confirmed after that (a card
//! form left open, a bank's slow 3-D Secure, a webhook delivered late) finds
//! them lapsed, and someone else may have taken them. So before the order is
//! placed the holds are taken again, which does nothing while they stand and
//! takes what is left otherwise; when the last of it is gone the payment goes
//! back ([`Outcome::Returned`]): an order is never placed for goods already
//! sold or a code already spent, and nothing is charged.
//!
//! A payment that buys nothing (that one, a second payment of a cart that is
//! an order already, one that does not hold what the cart was priced at) is
//! given back in three steps: the cart records that it goes back and why
//! (`Cart::PaymentReturnRequested`), Stripe releases the hold (or refunds
//! the money, had it been taken), and the cart records that it went
//! (`Cart::PaymentReturned`). Whatever comes after a step that did not finish
//! (Stripe telling again, its word that the hold went, the sweep
//! [`PendingPayments`](super::PendingPayments)) finishes it under the reason
//! recorded first.
//!
//! The dashboard takes the money itself (`settle`); Stripe saying so
//! (`payment_intent.succeeded`) is recorded unless known already, so a
//! capture whose recording failed, or one made in Stripe's own dashboard,
//! reaches the order; money taken for an order cancelled meanwhile goes back
//! through the order, the refund recorded before Stripe is asked
//! (`Order::RefundRequested`). A hold Stripe dropped
//! (`payment_intent.canceled`: lapsed after a week, or cancelled in its
//! dashboard) cancels the order.
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
use timada_orders::{CartState, OrderError, PROVIDER_STRIPE, Payment, Released};

use super::settle::{self, release_reason};
use super::{
    Account, DASHBOARD_ACTOR, PaymentState, PaymentStatus, Payments, Refused, ReleaseAnswer,
    signature,
};
use crate::cluster::Executor;

/// The refusal of a checkout that wants more than is left.
pub const OUT_OF_STOCK: &str = "out_of_stock";

/// The orders gateway of one tenant.
type Orders = timada_orders::Command<Executor>;

/// What the route answers Stripe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The order exists (now, or already), its payment held or taken.
    Placed { order_id: String },
    /// Taken for an order cancelled meanwhile: refunded through the order.
    Refunded { refund_id: String },
    /// Given back, no order placed with it (now, or already): a second
    /// payment of a cart, an amount that does not match, or confirmed too
    /// late, what the checkout held having lapsed and gone to someone else.
    /// `code` says which.
    Returned { code: String },
    /// To be given back, but Stripe will not, for good: settled as refused,
    /// for the merchant to settle in Stripe. `reason` is what Stripe said.
    NotReturned { reason: String },
    /// The order's payment is taken (recorded now, or already).
    Paid { order_id: String },
    /// The order's hold was dropped by Stripe: it is cancelled.
    Cancelled { order_id: String },
    /// The order's hold was dropped by Stripe while its supplier may have
    /// the goods: recorded, and the order left open for the merchant.
    HoldDropped { order_id: String },
    /// A `charge.refunded` matched against what the order knows: that many
    /// refunds Stripe holds were not recorded yet and are now.
    Reconciled { recorded: usize },
    /// Not ours to act on: another event type, a payment not made through
    /// timada, one the shopper has not confirmed.
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
        "payment_intent.amount_capturable_updated"
        | "payment_intent.succeeded"
        | "payment_intent.canceled" => event.data.object.id.clone(),
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
    let Some(cart_id) = status.metadata.get("cart_id").filter(|_| ours).cloned() else {
        return Ok(Outcome::Ignored);
    };
    if event.type_ == "charge.refunded" {
        return reconcile_refunds(payments, account, tenant_id, store, &cart_id, &payment_id).await;
    }
    reconcile(payments, account, tenant_id, store, &cart_id, &status).await
}

/// Brings the cart a payment was opened for, and its order, in step with
/// the payment as Stripe says it stands (`status`): a confirmed payment
/// makes the order, a taken one is recorded, a dropped hold cancels it, and
/// one that buys nothing goes back. What the webhook does for any of a
/// payment's events, and what the sweeps do for a payment whose webhook
/// never came, or whose return did not finish.
pub async fn reconcile(
    payments: &Payments,
    account: &Account,
    tenant_id: &str,
    store: Executor,
    cart_id: &str,
    status: &PaymentStatus,
) -> anyhow::Result<Outcome> {
    let orders = timada_orders::Command::new(store.clone(), tenant_id);
    let order = orders.load_order_of_cart(cart_id).await?;
    let own = order
        .as_ref()
        .filter(|order| order.payment.payment_id == status.payment_id);
    match status.state {
        PaymentState::Open | PaymentState::Processing => Ok(Outcome::Ignored),
        PaymentState::Cancelled => {
            if let Some(order) = own {
                if order.is_paid() || order.is_settled() || order.released_at.is_some() {
                    return Ok(Outcome::Ignored);
                }
                let released = orders
                    .record_release(
                        DASHBOARD_ACTOR,
                        &order.id,
                        &status.payment_id,
                        release_reason(status.lapsed),
                    )
                    .await?;
                let order_id = order.id.clone();
                return Ok(match released {
                    Released::Cancelled => {
                        tracing::info!(%tenant_id, order = %order_id, lapsed = status.lapsed, "an order's hold was dropped by Stripe: cancelled");
                        Outcome::Cancelled { order_id }
                    }
                    // The supplier may be shipping it: cancelling it is the
                    // merchant's call, who settles it with the shopper.
                    Released::KeptOpen => {
                        tracing::error!(%tenant_id, order = %order_id, lapsed = status.lapsed, "an order's hold was dropped by Stripe and its supplier may have it: left open for the merchant");
                        Outcome::HoldDropped { order_id }
                    }
                });
            }
            // No order: a payment given back already says so again, one on
            // its way back is recorded gone under the reason it went for;
            // otherwise the card form can confirm nothing any more.
            let cart = orders.load_cart(cart_id).await?;
            if let Some(returned) = cart
                .as_ref()
                .and_then(|cart| cart.returned_payment(&status.payment_id))
            {
                return Ok(Outcome::Returned {
                    code: returned.code.clone(),
                });
            }
            if let Some(pending) = cart
                .as_ref()
                .and_then(|cart| cart.pending_return(&status.payment_id))
            {
                orders
                    .return_payment(DASHBOARD_ACTOR, cart_id, &status.payment_id, None)
                    .await?;
                return Ok(Outcome::Returned {
                    code: pending.code.clone(),
                });
            }
            match orders
                .cancel_payment(DASHBOARD_ACTOR, cart_id, &status.payment_id)
                .await
            {
                Ok(()) | Err(OrderError::PaymentNotFound) => Ok(Outcome::Ignored),
                Err(err) => Err(err.into()),
            }
        }
        PaymentState::Authorized | PaymentState::Captured => {
            let captured = status.state == PaymentState::Captured;
            if let Some(order) = own {
                if !captured {
                    return Ok(Outcome::Placed {
                        order_id: order.id.clone(),
                    });
                }
                if order.is_paid() {
                    return Ok(Outcome::Paid {
                        order_id: order.id.clone(),
                    });
                }
                return record_capture(payments, account, &orders, &order.id, status).await;
            }
            // Given back already: the same answer, nothing more. On its way
            // back: given back under the reason it went for.
            let cart = orders.load_cart(cart_id).await?;
            if let Some(returned) = cart
                .as_ref()
                .and_then(|cart| cart.returned_payment(&status.payment_id))
            {
                return Ok(Outcome::Returned {
                    code: returned.code.clone(),
                });
            }
            if let Some(pending) = cart
                .as_ref()
                .and_then(|cart| cart.pending_return(&status.payment_id))
            {
                let code = pending.code.clone();
                return give_payment_back(payments, account, &orders, cart_id, status, &code).await;
            }
            // One Stripe would not give back, for good: the merchant's now.
            if let Some(reason) = cart
                .as_ref()
                .and_then(|cart| cart.return_refusal(&status.payment_id))
            {
                return Ok(Outcome::NotReturned {
                    reason: reason.to_owned(),
                });
            }
            if let Some(order) = &order {
                // The cart is an order already, paid by another payment.
                tracing::warn!(%tenant_id, order = %order.id, payment = %status.payment_id, "a second payment of an ordered cart: given back");
                return give_payment_back(
                    payments,
                    account,
                    &orders,
                    cart_id,
                    status,
                    DUPLICATE_PAYMENT,
                )
                .await;
            }
            // Not an order yet: the holds its checkout took may have lapsed
            // since, so they are taken again before the order is placed.
            if let Some(cart) = cart.as_ref()
                && let Some(gone) =
                    retake_holds(&store, tenant_id, cart, &status.payment_id).await?
            {
                tracing::warn!(%tenant_id, payment = %status.payment_id, code = %gone, "a late payment finds what it held gone: given back");
                return give_payment_back(payments, account, &orders, cart_id, status, gone).await;
            }
            let payment = Payment {
                provider: PROVIDER_STRIPE.to_owned(),
                payment_id: status.payment_id.clone(),
                amount_cents: if captured {
                    status.amount_received
                } else {
                    status.amount_capturable
                },
                currency: status.currency.to_ascii_uppercase(),
                capture_before: status.capture_before,
            };
            match orders.place_order(DASHBOARD_ACTOR, cart_id, payment).await {
                Ok(placed) if captured => {
                    record_capture(payments, account, &orders, &placed.order_id, status).await
                }
                Ok(placed) => Ok(Outcome::Placed {
                    order_id: placed.order_id,
                }),
                Err(err @ (OrderError::DuplicatePayment { .. } | OrderError::AmountMismatch)) => {
                    tracing::warn!(%tenant_id, payment = %status.payment_id, %err, "giving a payment back");
                    let code = err.code().unwrap_or(DUPLICATE_PAYMENT);
                    give_payment_back(payments, account, &orders, cart_id, status, code).await
                }
                Err(err) => {
                    tracing::error!(%tenant_id, payment = %status.payment_id, %err, "a confirmed payment could not become an order");
                    Ok(Outcome::Retry)
                }
            }
        }
    }
}

/// Why a second payment of a cart that is an order already goes back.
const DUPLICATE_PAYMENT: &str = "duplicate_payment";

/// What Stripe answers a refund it will never make: asking again, whatever
/// is fixed meanwhile, gets the same answer. A charge already refunded is not
/// among them: asked again, nothing is left to refund and the return is
/// recorded.
const REFUND_REFUSED_FOR_GOOD: &[&str] = &[
    "charge_disputed",
    "refund_disputed_payment",
    "charge_not_refundable",
];

/// Records the money Stripe says it took for an order; an order cancelled
/// in between gets it back.
async fn record_capture(
    payments: &Payments,
    account: &Account,
    orders: &Orders,
    order_id: &str,
    status: &PaymentStatus,
) -> anyhow::Result<Outcome> {
    match orders
        .record_payment(
            DASHBOARD_ACTOR,
            order_id,
            &status.payment_id,
            status.amount_received,
        )
        .await
    {
        Ok(()) => Ok(Outcome::Paid {
            order_id: order_id.to_owned(),
        }),
        Err(OrderError::OrderCancelled) => {
            tracing::warn!(tenant = %orders.tenant(), order = %order_id, payment = %status.payment_id, "money taken for a cancelled order: refunded");
            refund_cancelled(payments, account, orders, order_id, status).await
        }
        Err(err) => Err(err.into()),
    }
}

/// Gives back money Stripe took for an order cancelled meanwhile, as a
/// cancellation's refund is: asked for on the order first
/// (`Order::RefundRequested`, which fixes the refund's key), then made, then
/// recorded (`Order::Refunded`). One Stripe does not answer now stands for
/// the sweep.
async fn refund_cancelled(
    payments: &Payments,
    account: &Account,
    orders: &Orders,
    order_id: &str,
    status: &PaymentStatus,
) -> anyhow::Result<Outcome> {
    let pending = orders
        .request_cancelled_refund(
            DASHBOARD_ACTOR,
            order_id,
            &status.payment_id,
            status.amount_received,
        )
        .await?;
    let Some(pending) = pending else {
        // Gone back already: the same answer.
        let refund_id = orders
            .load_order(order_id)
            .await?
            .and_then(|order| order.refund_ids.last().cloned())
            .unwrap_or_default();
        return Ok(Outcome::Refunded { refund_id });
    };
    let refund_id = settle::refund(payments, account, orders, order_id, &pending).await?;
    Ok(Outcome::Refunded { refund_id })
}

/// Gives back a payment of the cart that buys nothing, `code` saying why: the
/// cart records that it goes back first (`Cart::PaymentReturnRequested`; a
/// return asked already keeps its own reason), then Stripe gives it back,
/// then the cart records that it went (`Cart::PaymentReturned`).
pub async fn give_payment_back(
    payments: &Payments,
    account: &Account,
    orders: &Orders,
    cart_id: &str,
    status: &PaymentStatus,
    code: &str,
) -> anyhow::Result<Outcome> {
    let pending = orders
        .request_payment_return(DASHBOARD_ACTOR, cart_id, &status.payment_id, code)
        .await?;
    let Some(pending) = pending else {
        // Settled already: the same answer.
        let cart = orders.load_cart(cart_id).await?;
        if let Some(reason) = cart
            .as_ref()
            .and_then(|cart| cart.return_refusal(&status.payment_id))
        {
            return Ok(Outcome::NotReturned {
                reason: reason.to_owned(),
            });
        }
        let code = cart
            .as_ref()
            .and_then(|cart| cart.returned_payment(&status.payment_id))
            .map_or_else(|| code.to_owned(), |returned| returned.code.clone());
        return Ok(Outcome::Returned { code });
    };
    let refund_id = match release_or_refund(payments, account, status).await {
        Ok(refund_id) => refund_id,
        // Stripe will not, for good (a disputed charge): the return is
        // settled as refused, for the merchant to settle in Stripe, rather
        // than asked again for ever. Any other refusal (a key that may not
        // refund, an amount that moved meanwhile) stands for a retry.
        Err(err)
            if err
                .downcast_ref::<Refused>()
                .and_then(|refused| refused.code.as_deref())
                .is_some_and(|code| REFUND_REFUSED_FOR_GOOD.contains(&code)) =>
        {
            let reason = err.to_string();
            orders
                .refuse_payment_return(DASHBOARD_ACTOR, cart_id, &status.payment_id, &reason)
                .await?;
            tracing::error!(tenant = %orders.tenant(), %cart_id, payment = %status.payment_id, %reason, "Stripe will not give a payment back: the merchant settles it in Stripe");
            return Ok(Outcome::NotReturned { reason });
        }
        Err(err) => return Err(err),
    };
    orders
        .return_payment(DASHBOARD_ACTOR, cart_id, &status.payment_id, refund_id)
        .await?;
    Ok(Outcome::Returned { code: pending.code })
}

/// Asks Stripe to give a payment back: its hold released, or, had it been
/// taken, what is not refunded yet (by hand in Stripe's own dashboard, say)
/// refunded under a key of the payment and that amount. The refund's id,
/// when there is one.
async fn release_or_refund(
    payments: &Payments,
    account: &Account,
    status: &PaymentStatus,
) -> anyhow::Result<Option<String>> {
    let mut amount_cents = status.amount_received;
    if status.state != PaymentState::Captured {
        match payments
            .gateway
            .release_payment(&account.secret_key, &status.payment_id)
            .await?
        {
            ReleaseAnswer::Released => return Ok(None),
            ReleaseAnswer::Captured {
                amount_cents: taken,
            } => amount_cents = taken,
        }
    }
    let refunds = payments
        .gateway
        .refunds_of(&account.secret_key, &status.payment_id)
        .await?;
    let refunded: u64 = refunds
        .iter()
        .filter(|refund| refund.succeeded)
        .map(|refund| refund.amount_cents)
        .sum();
    let left = amount_cents.saturating_sub(refunded);
    if left == 0 {
        return Ok(refunds
            .iter()
            .rev()
            .find(|refund| refund.succeeded)
            .map(|refund| refund.refund_id.clone()));
    }
    let key = format!("timada-return-{}-{left}", status.payment_id);
    let refund_id = payments
        .gateway
        .refund(&account.secret_key, &status.payment_id, left, &key)
        .await?;
    Ok(Some(refund_id))
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
/// `place_order` says what is wrong with it.
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
