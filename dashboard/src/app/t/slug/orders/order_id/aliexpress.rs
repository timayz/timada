use serde::Deserialize;
use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{OrderId, order_url};
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
pub struct ForwardForm {
    /// Set once the merchant looked on AliExpress: places an order whose
    /// last attempt never said how it went.
    again: Option<String>,
}

/// Places the order's AliExpress items on AliExpress; the dashboard does
/// it a moment later.
#[route(POST)]
pub async fn forward(cx: &Cx, Form(form): Form<ForwardForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let order_id = path_param::<OrderId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let forwards = timada_aliexpress::Command::new(access.executor()?.clone(), access.tenant_id());
    match forwards
        .request_forward(&access.user_id, &order_id, form.again.is_some())
        .await
    {
        Ok(_) => Ok(see_other(order_url(
            cx,
            &slug,
            &order_id,
            None,
            Some("The order is being placed on AliExpress."),
        ))),
        Err(err) => match err.code() {
            Some(code) => Ok(see_other(order_url(cx, &slug, &order_id, Some(code), None))),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
