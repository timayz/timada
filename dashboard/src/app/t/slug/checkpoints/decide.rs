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

use super::{checkpoints_url, launches};
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
pub struct DecideForm {
    #[serde(default)]
    checkpoint: u32,
    #[serde(default)]
    decision: String,
    #[serde(default)]
    note: String,
}

/// Records what the merchant decided at a checkpoint, and why. Nothing
/// follows from it by itself: a "stop" switches nothing off.
#[route(POST)]
pub async fn decide(cx: &Cx, Form(form): Form<DecideForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match launches(&access)?
        .record_decision(&access.user_id, form.checkpoint, &form.decision, &form.note)
        .await
    {
        Ok(()) => Ok(see_other(checkpoints_url(
            cx,
            slug,
            None,
            Some("Decision recorded."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(checkpoints_url(cx, slug, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
