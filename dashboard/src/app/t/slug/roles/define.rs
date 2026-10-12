use std::collections::HashMap;

use timada_tenancy::DefineRole;
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

/// Defines a custom role from the form: `role`, `label` and one `perm:…`
/// field per ticked permission. The domain checks `roles:manage`.
#[route(POST)]
pub async fn define(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    let cmd = DefineRole {
        role: form.get("role").cloned().unwrap_or_default(),
        label: form.get("label").cloned().unwrap_or_default(),
        permissions: checked_permissions(&form),
    };
    match tenancy(cx)
        .define_role(&access.user_id, &access.tenant.id, cmd)
        .await
    {
        Ok(()) => Ok(see_other(roles_url(cx, slug, None, Some("Role defined.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(roles_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
