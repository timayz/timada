use std::collections::HashMap;

use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{checked_permissions, roles_url};
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Replaces a role's label and permissions with what the form says.
#[route(POST)]
pub async fn update(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    let role = form.get("role").cloned().unwrap_or_default();
    let label = form.get("label").cloned().unwrap_or_default();
    match tenancy(cx)
        .update_role(
            &access.user_id,
            &access.tenant.id,
            &role,
            &label,
            checked_permissions(&form),
        )
        .await
    {
        Ok(()) => Ok(see_other(roles_url(cx, slug, None, Some("Role saved.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(roles_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
