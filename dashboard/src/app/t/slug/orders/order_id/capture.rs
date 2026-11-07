use std::sync::Arc;

use timada_orders::{CaptureClaim, OrderError};
use timada_tenancy::PERMISSION_ORDERS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{OrderId, order_url};
use crate::app::t::slug::orders::orders;
use crate::app::t::slug::require_tenant;
use crate::payments::Payments;
use crate::payments::settle::{CaptureOutcome, capture as take};

/// Takes the money held on the shopper's card for the order, when the
/// merchant says so: before buying its goods by hand, or once more after
/// Stripe refused. A request already standing is settled rather than asked
/// again.
#[route(POST)]
pub async fn capture(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_MANAGE)
        .await?;
    let order_id = path_param::<OrderId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let back = |error: Option<&str>, message: Option<&str>| {
        see_other(order_url(cx, &slug, &order_id, error, message))
    };
    let payments = app_context::<Arc<Payments>>(cx);
    let Some(account) = payments
        .account_of(&access.tenant)
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Ok(back(Some("payments_unavailable"), None));
    };
    let gateway = orders(&access)?;
    match gateway.request_capture(&access.user_id, &order_id).await {
        Ok(CaptureClaim::Paid) => return Ok(back(None, Some("The payment was taken."))),
        Ok(CaptureClaim::Claimed(_) | CaptureClaim::Pending(_)) => {}
        Err(err @ OrderError::Other(_)) => return Err(topcoat::Error::from_anyhow(err.into())),
        Err(err) => match err.code() {
            Some(code) => return Ok(back(Some(code), None)),
            None => return Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
    let taken = take(payments, &account, &gateway, &order_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(match taken {
        CaptureOutcome::Paid | CaptureOutcome::Nothing => {
            back(None, Some("The payment was taken."))
        }
        CaptureOutcome::Released => back(Some("hold_expired"), None),
        CaptureOutcome::KeptOpen => back(Some("hold_expired_supplier_ordered"), None),
        CaptureOutcome::Refused(_) => back(Some("capture_failed"), None),
        CaptureOutcome::Unanswered => back(Some("payments_unavailable"), None),
    })
}
