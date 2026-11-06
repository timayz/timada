use std::sync::Arc;

use timada_orders::{CancelPath, OrderError, normalize_reason};
use timada_tenancy::PERMISSION_ORDERS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{CancelForm, OrderId, order_url};
use crate::app::t::slug::orders::orders;
use crate::app::t::slug::require_tenant;
use crate::payments::Payments;
use crate::payments::settle::{CancelOutcome, settle_cancellation, settle_released_cancellation};

/// Calls off an order that has not left. The cancellation is asked for on
/// the order first, then settled with Stripe: a payment only held is given
/// back, one taken is refunded in full (the refund asked for on the order
/// first, which fixes the idempotency key), and only then is the order
/// cancelled. A Stripe that refuses, or does not answer, leaves the
/// cancellation standing: posting again retries under the same key, so
/// Stripe never refunds twice, and the sweep settles it otherwise. An order
/// whose hold went back already (kept open, AliExpress perhaps holding it)
/// holds nothing: it is cancelled without Stripe, the shop's keys there or
/// not.
#[route(POST)]
pub async fn cancel(cx: &Cx, Form(form): Form<CancelForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_MANAGE)
        .await?;
    let order_id = path_param::<OrderId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let back = |error: Option<&str>, message: Option<&str>| {
        see_other(order_url(cx, &slug, &order_id, error, message))
    };
    let reason = match normalize_reason(&form.reason) {
        Ok(reason) => reason,
        Err(err) => return Ok(back(err.code(), None)),
    };
    let payments = app_context::<Arc<Payments>>(cx);
    let account = payments
        .account_of(&access.tenant)
        .map_err(topcoat::Error::from_anyhow)?;
    let gateway = orders(&access)?;
    // Nothing is held of an order kept open once its hold went back: it is
    // cancelled without the provider, its keys there or not.
    let holds_nothing = gateway
        .load_order(&order_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some_and(|order| order.released_at.is_some() && !order.is_paid());
    if account.is_none() && !holds_nothing {
        return Ok(back(Some("payments_unavailable"), None));
    }
    let path = match gateway
        .request_cancellation(
            &access.user_id,
            &order_id,
            reason,
            form.supplier_cancelled.is_some(),
        )
        .await
    {
        Ok(path) => path,
        Err(err @ OrderError::Other(_)) => return Err(topcoat::Error::from_anyhow(err.into())),
        Err(err) => match err.code() {
            Some(code) => return Ok(back(Some(code), None)),
            None => return Err(topcoat::Error::from_anyhow(err.into())),
        },
    };
    if settle_released_cancellation(&gateway, &order_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some()
    {
        return Ok(back(None, Some("Order cancelled.")));
    }
    let Some(account) = account else {
        return Ok(back(Some("payments_unavailable"), None));
    };
    let settled = settle_cancellation(payments, &account, &gateway, &order_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(match settled {
        CancelOutcome::Cancelled | CancelOutcome::Nothing => back(
            None,
            Some(match path {
                CancelPath::Refund => "Order cancelled and refunded.",
                CancelPath::Release { .. } => {
                    "Order cancelled: the hold on the shopper's card was given back."
                }
                CancelPath::Done => "Order cancelled.",
            }),
        ),
        CancelOutcome::Unanswered(reason) => {
            tracing::warn!(tenant = %slug, %order_id, %reason, "an order's cancellation is not settled with Stripe yet");
            back(Some("refund_failed"), None)
        }
    })
}
