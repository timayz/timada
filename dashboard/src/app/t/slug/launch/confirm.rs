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
pub struct ConfirmForm {
    #[serde(default)]
    step: String,
    #[serde(default)]
    holds: String,
}

/// Says that a step timada cannot check holds, or takes that word back.
/// The page keeps who said so.
#[route(POST)]
pub async fn confirm(cx: &Cx, Form(form): Form<ConfirmForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let holds = form.holds == "yes";
    match timada_launch::Command::new(access.executor()?.clone(), access.tenant_id())
        .confirm_step(&access.user_id, &form.step, holds)
        .await
    {
        Ok(()) => Ok(see_other(launch_url(
            cx,
            slug,
            None,
            Some(if holds {
                "Noted: it holds."
            } else {
                "Taken back."
            }),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(launch_url(cx, slug, err.code(), None))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
