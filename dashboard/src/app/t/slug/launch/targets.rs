use serde::Deserialize;
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

use super::launch_url;
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
pub struct TargetsForm {
    #[serde(default)]
    per_category: u32,
    #[serde(default)]
    products: u32,
}

/// Sets how many products the path holds every category and the shop to.
#[route(POST)]
pub async fn targets(cx: &Cx, Form(form): Form<TargetsForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match timada_launch::Command::new(access.executor()?.clone(), access.tenant_id())
        .set_path_targets(&access.user_id, form.per_category, form.products)
        .await
    {
        Ok(()) => Ok(see_other(launch_url(cx, slug, None, Some("Saved.")))),
        Err(err) if err.is_user_facing() => Ok(see_other(launch_url(cx, slug, err.code(), None))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
