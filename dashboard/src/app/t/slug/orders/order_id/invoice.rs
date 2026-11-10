use timada_orders::OrderError;
use timada_tenancy::PERMISSION_ORDERS_MANAGE;
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

/// Asks for the order's documents to be issued again (the invoice it has
/// none of, the credit notes its refunds have none of), after the dashboard
/// gave up on them (`Order::InvoicingFailed`): the request is recorded on
/// the order and the invoicing saga follows it, so the page shows the
/// documents a moment later.
#[route(POST)]
pub async fn invoice(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_MANAGE)
        .await?;
    let order_id = path_param::<OrderId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let back = |error: Option<&str>, message: Option<&str>| {
        see_other(order_url(cx, &slug, &order_id, error, message))
    };
    match orders(&access)?
        .request_invoicing(&access.user_id, &order_id)
        .await
    {
        Ok(()) => Ok(back(
            None,
            Some("The documents are being issued: reload in a moment."),
        )),
        Err(err @ OrderError::Other(_)) => Err(topcoat::Error::from_anyhow(err.into())),
        Err(err) => match err.code() {
            Some(code) => Ok(back(Some(code), None)),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
