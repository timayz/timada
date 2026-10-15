use timada_tenancy::PERMISSION_TENANT_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::invoicing_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

#[derive(serde::Deserialize)]
pub struct LinkForm {
    company_id: String,
    series_code: String,
}

/// Makes the tenant sell as one of the member's companies.
#[route(POST)]
pub async fn link(cx: &Cx, Form(form): Form<LinkForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .link_tenant(
            &access.user_id,
            &form.company_id,
            &access.tenant.id,
            Some(&form.series_code),
        )
        .await
    {
        Ok(()) => Ok(see_other(invoicing_url(
            cx,
            slug,
            None,
            Some("Linked to the company."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(invoicing_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
