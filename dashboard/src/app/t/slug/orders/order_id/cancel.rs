use std::sync::Arc;

use timada_orders::{OrderStatus, normalize_reason};
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

use super::{CancelForm, OrderId, order_url, require_order};
use crate::app::t::slug::orders::orders;
use crate::app::t::slug::require_tenant;
use crate::payments::Payments;

/// Calls off a paid order: the payment goes back through Stripe first (in
/// full, what is not refunded yet), then the order is cancelled. A refund
/// Stripe refuses leaves the order as it was.
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
    let order = require_order(cx, &access).await?;
    let gateway = orders(&access)?;
    let outstanding = order.total_cents.saturating_sub(order.refunded_cents);
    if order.status == OrderStatus::Paid && outstanding > 0 {
        let payments = app_context::<Arc<Payments>>(cx);
        let Some(account) = payments
            .account_of(&access.tenant)
            .map_err(topcoat::Error::from_anyhow)?
        else {
            return Ok(back(Some("payments_unavailable"), None));
        };
        let refund = payments
            .gateway
            .refund(
                &account.secret_key,
                &order.payment.payment_id,
                outstanding,
                &format!("timada-cancel-{order_id}"),
            )
            .await;
        let refund_id = match refund {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(tenant = %slug, %order_id, %err, "Stripe refused a refund");
                return Ok(back(Some("refund_failed"), None));
            }
        };
        gateway
            .record_refund(&access.user_id, &order_id, &refund_id, outstanding)
            .await
            .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
    }
    match gateway
        .cancel_order(&access.user_id, &order_id, reason)
        .await
    {
        Ok(()) => Ok(back(None, Some("Order cancelled and refunded."))),
        Err(err) => match err.code() {
            Some(code) => Ok(back(Some(code), None)),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
