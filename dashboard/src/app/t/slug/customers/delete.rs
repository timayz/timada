//! `POST /t/{slug}/customers/delete` (`shopper_id=…`), for members with
//! `customers:manage`: deletes a shopper's account, as an erasure request
//! asks. Their addresses are forgotten and every session of theirs ends; the
//! orders placed with the address stay with the shop. Their address leaves
//! the shop's newsletter too ([`crate::shopper_erasure`]). Unknown or
//! already deleted is a no-op.

use serde::Deserialize;
use timada_tenancy::PERMISSION_CUSTOMERS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{customers_url, error_code};
use crate::app::store::shoppers::newsletter_subscriber_of;
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
struct DeleteForm {
    shopper_id: String,
}

#[route(POST)]
pub async fn delete(cx: &Cx, Form(form): Form<DeleteForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_CUSTOMERS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let shoppers = timada_shoppers::Command::new(access.executor()?.clone(), access.tenant_id());
    // Read before the key goes: the newsletter knows them by their address.
    let subscriber = match shoppers
        .load_shopper(&form.shopper_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    {
        Some(shopper) => {
            newsletter_subscriber_of(cx, access.executor()?, access.tenant_id(), &shopper).await?
        }
        None => None,
    };
    match shoppers
        .forget(
            &*crate::app::store::shoppers::keys(cx, &access.tenant.id),
            &form.shopper_id,
            subscriber.as_deref(),
        )
        .await
    {
        Ok(()) => Ok(see_other(customers_url(
            cx,
            slug,
            None,
            Some("Customer deleted; the list updates in a moment."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(customers_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
