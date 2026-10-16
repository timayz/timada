use timada_tenancy::PERMISSION_SHIPPING_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{MethodForm, error_code, orders, shipping_url};
use crate::app::t::slug::require_tenant;

/// Adds a shipping method and lands back on the list.
#[route(POST)]
pub async fn add(cx: &Cx, Form(form): Form<MethodForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SHIPPING_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let added = match form.method() {
        Ok(method) => {
            orders(&access)?
                .add_shipping_method(&access.user_id, method)
                .await
        }
        Err(err) => Err(err),
    };
    match added {
        Ok(_) => Ok(see_other(shipping_url(
            cx,
            slug,
            None,
            Some("Shipping method added."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(shipping_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
