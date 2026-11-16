use serde::Deserialize;
use timada_orders::{OrderError, ReturnTerms};
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

use super::{error_code, orders, shipping_url};
use crate::app::t::slug::require_tenant;

/// The return terms as the merchant fills them in; a box left unticked
/// posts nothing.
#[derive(Deserialize)]
pub struct ReturnsForm {
    #[serde(default)]
    days: String,
    #[serde(default)]
    free: String,
}

/// States the shop's return terms, or takes them back when the days are
/// left empty.
#[route(POST)]
pub async fn returns(cx: &Cx, Form(form): Form<ReturnsForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SHIPPING_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let days = form.days.trim();
    let terms = if days.is_empty() {
        Ok(None)
    } else {
        days.parse()
            .map(|days| {
                Some(ReturnTerms {
                    days,
                    free: !form.free.is_empty(),
                })
            })
            .map_err(|_| OrderError::InvalidReturnDays)
    };
    let set = match terms {
        Ok(terms) => orders(&access)?
            .set_return_policy(&access.user_id, terms)
            .await
            .map(|()| terms),
        Err(err) => Err(err),
    };
    match set {
        Ok(Some(_)) => Ok(see_other(shipping_url(
            cx,
            slug,
            None,
            Some("Return terms set."),
        ))),
        Ok(None) => Ok(see_other(shipping_url(
            cx,
            slug,
            None,
            Some("No return terms stated."),
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
