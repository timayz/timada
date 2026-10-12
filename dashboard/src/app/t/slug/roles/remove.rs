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

use super::roles_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

#[derive(Deserialize)]
struct RemoveForm {
    role: String,
}

/// Removes a custom role nobody holds.
#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<RemoveForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .remove_role(&access.user_id, &access.tenant.id, &form.role)
        .await
    {
        Ok(()) => Ok(see_other(roles_url(cx, slug, None, Some("Role removed.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(roles_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
