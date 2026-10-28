use timada_tenancy::PERMISSION_AUTOMATIONS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::super::automations;
use super::{ScenarioId, scenario_url};
use crate::app::t::slug::require_tenant;

/// Runs a held scenario again; the runs that waited carry on.
#[route(POST)]
pub async fn resume(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_MANAGE)
        .await?;
    let id = path_param::<ScenarioId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    match automations(&access)?
        .resume_scenario(&access.user_id, &id)
        .await
    {
        Ok(()) => Ok(see_other(scenario_url(
            cx,
            &slug,
            &id,
            None,
            Some("Running again."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(scenario_url(cx, &slug, &id, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
