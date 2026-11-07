use timada_tenancy::PERMISSION_PAYMENTS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::{payments_url, publish_setup};
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};
use crate::db::Db;

/// Forgets the keys of a tenant that stopped taking payments, at the
/// merchant's word. Refused while the orders show anything held or owed
/// (`money_owed`), and by the tenancy until the stop is eight days old, past
/// the longest hold a card keeps.
///
/// The orders are read from their read model, which may trail the store (a
/// node catching up after a restart, a worker that stopped): it can still
/// miss a refund or a payment given back that stands. No card holds money
/// that long after a stop, and saving the keys again resumes their
/// settlement, so forgetting too early costs time, never money.
#[route(POST)]
pub async fn forget(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAYMENTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let owed = timada_orders::money_owed(app_context::<Db>(cx), &access.tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if owed > 0 {
        return Ok(see_other(payments_url(cx, slug, Some("money_owed"), None)));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    match tenancy(cx)
        .forget_payments(&access.user_id, &access.tenant.id, now)
        .await
    {
        Ok(()) => {
            publish_setup(cx, &access).await;
            Ok(see_other(payments_url(
                cx,
                slug,
                None,
                Some("Keys forgotten."),
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
