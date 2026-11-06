use timada_aliexpress::{ALIEXPRESS_SUPPLIER, FORWARD_STALL_SECS, ForwardState, ForwardStatus};
use timada_orders::{OrderError, OrderState, OrderStatus};
use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{OrderId, order_url};
use crate::app::t::slug::orders::orders;
use crate::app::t::slug::require_tenant;

/// Why the merchant may not say now that the order is not on AliExpress, as
/// a page code; `None` when they may. An attempt must have never said how it
/// went, nothing be asking AliExpress now, and the order's forward have
/// failed (one placed is there, whatever the order says). AliExpress's own
/// list may take a while to show an order it took, so the merchant is asked
/// only [`FORWARD_STALL_SECS`] after the order became uncertain, at `now`
/// (unix seconds). Nothing uncertain is nothing to refuse: `None`.
pub fn rule_out_refusal(
    order: &OrderState,
    forward: Option<&ForwardState>,
    now: u64,
) -> Option<&'static str> {
    if !order.supplier_uncertain
        || !matches!(order.status, OrderStatus::Authorized | OrderStatus::Paid)
    {
        return None;
    }
    match forward.map(|forward| forward.status) {
        Some(ForwardStatus::Placed | ForwardStatus::Done) => {
            return Some("aliexpress_already_forwarded");
        }
        Some(ForwardStatus::Requested | ForwardStatus::Started) => {
            return Some("aliexpress_forwarding");
        }
        Some(ForwardStatus::Failed) | None => {}
    }
    if order.supplier_attempt_open {
        return Some("aliexpress_forwarding");
    }
    let since = order.supplier_uncertain_at.unwrap_or(order.updated_at);
    if now.saturating_sub(since) <= FORWARD_STALL_SECS {
        return Some("aliexpress_list_catching_up");
    }
    None
}

/// The merchant looked at the account's orders on AliExpress and this one
/// is not there: what an attempt that never said how it went may have
/// placed is ruled out, and the shopper may cancel the order again.
#[route(POST)]
pub async fn not_on_aliexpress(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let order_id = path_param::<OrderId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let back = |error: Option<&str>, message: Option<&str>| {
        see_other(order_url(cx, &slug, &order_id, error, message))
    };
    let gateway = orders(&access)?;
    let Some(order) = gateway
        .load_order(&order_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Ok(back(Some("order_not_found"), None));
    };
    if !order.supplier_uncertain {
        return Ok(back(None, None));
    }
    // The page may be stale: the forward says whether AliExpress has it.
    let forward = timada_aliexpress::Command::new(access.executor()?.clone(), access.tenant_id())
        .load_forward(&order_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if let Some(code) = rule_out_refusal(&order, forward.as_ref(), crate::auth::now()) {
        return Ok(back(Some(code), None));
    }
    match gateway
        .rule_out_supplier_order(&access.user_id, &order_id, ALIEXPRESS_SUPPLIER)
        .await
    {
        Ok(()) => Ok(back(
            None,
            Some("Noted: the order is not on AliExpress, and the shopper may cancel it again."),
        )),
        Err(err @ OrderError::Other(_)) => Err(topcoat::Error::from_anyhow(err.into())),
        Err(err) => match err.code() {
            Some(code) => Ok(back(Some(code), None)),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}

#[cfg(test)]
mod tests {
    use timada_orders::SupplierOrder;

    use super::*;

    const NOW: u64 = 1_000_000;

    fn uncertain(at: u64) -> OrderState {
        OrderState {
            status: OrderStatus::Authorized,
            supplier_order: SupplierOrder::Started,
            supplier_uncertain: true,
            supplier_uncertain_at: Some(at),
            ..Default::default()
        }
    }

    fn forward(status: ForwardStatus) -> ForwardState {
        ForwardState {
            status,
            ..Default::default()
        }
    }

    #[test]
    fn ruled_out_once_the_forward_failed_and_the_list_had_time() {
        let long_ago = NOW - FORWARD_STALL_SECS - 1;
        let failed = forward(ForwardStatus::Failed);
        assert_eq!(
            rule_out_refusal(&uncertain(long_ago), Some(&failed), NOW),
            None
        );
        assert_eq!(
            rule_out_refusal(&uncertain(NOW - 60), Some(&failed), NOW),
            Some("aliexpress_list_catching_up")
        );
    }

    #[test]
    fn never_ruled_out_while_aliexpress_has_it_or_is_asked() {
        let long_ago = NOW - FORWARD_STALL_SECS - 1;
        for (status, code) in [
            (ForwardStatus::Placed, "aliexpress_already_forwarded"),
            (ForwardStatus::Done, "aliexpress_already_forwarded"),
            (ForwardStatus::Requested, "aliexpress_forwarding"),
            (ForwardStatus::Started, "aliexpress_forwarding"),
        ] {
            assert_eq!(
                rule_out_refusal(&uncertain(long_ago), Some(&forward(status)), NOW),
                Some(code),
                "{status:?}"
            );
        }
        let asking = OrderState {
            supplier_attempt_open: true,
            ..uncertain(long_ago)
        };
        assert_eq!(
            rule_out_refusal(&asking, Some(&forward(ForwardStatus::Failed)), NOW),
            Some("aliexpress_forwarding")
        );
    }

    #[test]
    fn nothing_uncertain_is_nothing_to_refuse() {
        let settled = OrderState {
            status: OrderStatus::Authorized,
            ..Default::default()
        };
        assert_eq!(
            rule_out_refusal(&settled, Some(&forward(ForwardStatus::Placed)), NOW),
            None
        );
    }
}
