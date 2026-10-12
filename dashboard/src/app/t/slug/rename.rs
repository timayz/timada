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
struct RenameForm {
    name: String,
}

/// Members with `tenant:manage` rename the tenant; the domain refuses everyone else.
#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .rename_tenant(&user.id, &access.tenant.id, &form.name)
        .await
    {
        Ok(()) => Ok(see_other(tenant_url(
            cx,
            slug,
            None,
            Some("Tenant renamed."),
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
