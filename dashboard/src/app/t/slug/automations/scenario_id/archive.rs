use timada_tenancy::PERMISSION_AUTOMATIONS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::super::{automations, automations_url};
use super::{ScenarioId, scenario_url};
use crate::app::t::slug::require_tenant;

/// Deletes the scenario from the list: nothing runs from it any more.
#[route(POST)]
pub async fn archive(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_MANAGE)
        .await?;
    let id = path_param::<ScenarioId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    match automations(&access)?
        .archive_scenario(&access.user_id, &id)
        .await
    {
        Ok(()) => Ok(see_other(automations_url(
            cx,
            &slug,
            None,
            Some("Scenario deleted."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(scenario_url(cx, &slug, &id, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
