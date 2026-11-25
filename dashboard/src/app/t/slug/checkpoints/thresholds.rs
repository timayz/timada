use std::collections::HashMap;

use timada_launch::THRESHOLDS;
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

use super::{checkpoints_url, launches};
use crate::app::t::slug::require_tenant;

/// Sets the thresholds the shop's checkpoints are judged on.
#[route(POST)]
pub async fn thresholds(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let set: Vec<(String, String)> = THRESHOLDS
        .iter()
        .filter_map(|(name, _)| Some(((*name).to_owned(), form.get(*name)?.clone())))
        .collect();
    match launches(&access)?
        .set_thresholds(&access.user_id, set)
        .await
    {
        Ok(()) => Ok(see_other(checkpoints_url(
            cx,
            slug,
            None,
            Some("Thresholds saved."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(checkpoints_url(cx, slug, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
