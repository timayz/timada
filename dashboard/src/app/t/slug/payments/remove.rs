use timada_tenancy::PERMISSION_PAYMENTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::{payments_url, publish_setup};
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Stops taking payments: storefronts stop offering the card form at once
/// and new requests to pay are refused. The keys stay, sealed, while an
/// order holds money or anything is owed, so the sweeps and Stripe's
/// webhook still settle it; a sweep forgets them once nothing is. Saving
/// keys again takes payments again.
#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAYMENTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .stop_payments(&access.user_id, &access.tenant.id)
        .await
    {
        Ok(()) => {
            publish_setup(cx, &access).await;
            Ok(see_other(payments_url(
                cx,
                slug,
                None,
                Some("Payments stopped."),
            )))
        }
        Err(err) if err.is_user_facing() => Ok(see_other(payments_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
