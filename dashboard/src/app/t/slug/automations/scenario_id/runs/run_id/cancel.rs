use timada_tenancy::PERMISSION_AUTOMATIONS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::super::super::super::automations;
use super::super::super::ScenarioId;
use super::{RunId, run_url};
use crate::app::t::slug::require_tenant;

/// Stops a run under way: its next steps never happen.
#[route(POST)]
pub async fn cancel(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_MANAGE)
        .await?;
    let scenario_id = path_param::<ScenarioId>(cx).to_owned();
    let run_id = path_param::<RunId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let reason = "Cancelled by a member.";
    match automations(&access)?
        .cancel_run(&access.user_id, &run_id, reason)
        .await
    {
        Ok(_) => Ok(see_other(run_url(
            cx,
            &slug,
            &scenario_id,
            &run_id,
            None,
            Some("Run cancelled."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(run_url(
            cx,
            &slug,
            &scenario_id,
            &run_id,
            err.code(),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
