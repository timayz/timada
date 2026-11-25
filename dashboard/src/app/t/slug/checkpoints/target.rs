use serde::Deserialize;
use timada_launch::{LaunchError, parse_hundredths};
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
pub struct TargetForm {
    #[serde(default)]
    amount: String,
    #[serde(default)]
    currency: String,
    #[serde(default)]
    by_day: String,
}

/// Sets what the shop is to reach: a profit after ads a month, by a day.
/// The target before stays on the page, under it.
#[route(POST)]
pub async fn target(cx: &Cx, Form(form): Form<TargetForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let set = match parse_hundredths(&form.amount) {
        Some(cents) => {
            launches(&access)?
                .set_target(
                    &access.user_id,
                    cents,
                    &form.currency,
                    &form.by_day,
                    crate::auth::now(),
                )
                .await
        }
        None => Err(LaunchError::InvalidTarget),
    };
    match set {
        Ok(()) => Ok(see_other(checkpoints_url(
            cx,
            slug,
            None,
            Some("Target set."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(checkpoints_url(cx, slug, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
