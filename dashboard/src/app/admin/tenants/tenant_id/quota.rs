use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{TenantId, admin_tenant_url};
use crate::app::admin::require_admin;
use crate::app::tenants::{error_code, tenancy};

#[derive(Deserialize)]
pub struct QuotaForm {
    /// Blank puts the tenant back on the platform's default.
    quota_mib: String,
}

/// Sets how much disk the tenant's pictures may take.
#[route(POST)]
pub async fn quota(cx: &Cx, Form(form): Form<QuotaForm>) -> Result<SeeOther> {
    require_admin(cx).await?;
    let id = path_param::<TenantId>(cx);
    let mib = form.quota_mib.trim();
    let bytes = if mib.is_empty() {
        None
    } else {
        match mib.parse::<u64>() {
            Ok(mib) if mib > 0 => Some(mib * 1024 * 1024),
            _ => {
                return Ok(see_other(admin_tenant_url(
                    cx,
                    id,
                    Some("invalid_media_quota"),
                    None,
                )));
            }
        }
    };
    match tenancy(cx).set_media_quota(id, bytes).await {
        Ok(()) => Ok(see_other(admin_tenant_url(
            cx,
            id,
            None,
            Some(if bytes.is_some() {
                "Picture quota set."
            } else {
                "Picture quota back on the platform's default."
            }),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(admin_tenant_url(
            cx,
            id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
