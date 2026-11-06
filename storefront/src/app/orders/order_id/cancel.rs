use timada_storefront::cancel_order;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, not_found, see_other},
        path_param, route,
    },
};

use super::{OrderId, cancelled_url};
use crate::shopper::holds_order;

/// The shopper cancels their order, then back to its page: cancelled,
/// nothing charged, or why it no longer can be.
#[route(POST)]
pub async fn cancel(cx: &Cx) -> Result<SeeOther> {
    let order_id = path_param::<OrderId>(cx).to_owned();
    if !holds_order(cx, &order_id).await? {
        return Err(not_found().into());
    }
    let outcome = cancel_order(cx, &order_id).await?;
    let error = outcome.err().map(|refusal| refusal.code());
    Ok(see_other(cancelled_url(cx, &order_id, error.flatten())))
}
