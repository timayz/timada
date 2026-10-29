use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{require_tenant, tenant_url};
use crate::app::tenants::{error_code, tenancy};
use crate::auth::require_user;

#[derive(Deserialize)]
struct TimeZoneForm {
    time_zone: String,
}

/// Members with `tenant:manage` set where the shop lives; the domain refuses
/// everyone else.
#[route(POST)]
pub async fn time_zone(cx: &Cx, Form(form): Form<TimeZoneForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .set_time_zone(&user.id, &access.tenant.id, &form.time_zone)
        .await
    {
        Ok(()) => Ok(see_other(tenant_url(
            cx,
            slug,
            None,
            Some("Time zone saved."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(tenant_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
