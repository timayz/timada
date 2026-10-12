use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{TenantId, admin_tenant_url};
use crate::app::admin::require_admin;
use crate::app::tenants::{error_code, tenancy};
use crate::tenants::Tenants;

#[derive(Deserialize)]
struct SuspendForm {
    reason: String,
}

/// Takes the tenant offline: the catalog records it, then its database is
/// closed on this node (other nodes close theirs when they next look).
#[route(POST)]
pub async fn suspend(cx: &Cx, Form(form): Form<SuspendForm>) -> Result<SeeOther> {
    require_admin(cx).await?;
    let id = path_param::<TenantId>(cx);
    match tenancy(cx).suspend_tenant(id, &form.reason).await {
        Ok(()) => {
            app_context::<Arc<Tenants>>(cx).close(id).await;
            Ok(see_other(admin_tenant_url(
                cx,
                id,
                None,
                Some("Tenant suspended."),
            )))
        }
        Err(err) if err.is_user_facing() => Ok(see_other(admin_tenant_url(
            cx,
            id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
