use timada_aliexpress::ALIEXPRESS_SUPPLIER;
use timada_orders::OrderError;
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
    let gateway = orders(&access)?;
    match gateway
        .rule_out_supplier_order(&access.user_id, &order_id, ALIEXPRESS_SUPPLIER)
        .await
    {
        Ok(()) => Ok(see_other(order_url(
            cx,
            &slug,
            &order_id,
            None,
            Some("Noted: the order is not on AliExpress, and the shopper may cancel it again."),
        ))),
        Err(err @ OrderError::Other(_)) => Err(topcoat::Error::from_anyhow(err.into())),
        Err(err) => match err.code() {
            Some(code) => Ok(see_other(order_url(cx, &slug, &order_id, Some(code), None))),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
