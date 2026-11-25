use std::collections::HashMap;

use timada_google::TUNING_FIELDS;
use timada_tenancy::PERMISSION_ADS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::tuning_url;
use crate::app::t::slug::google::google;
use crate::app::t::slug::require_tenant;

/// Sets the thresholds the shop's rounds decide on.
#[route(POST)]
pub async fn set(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_ADS_MANAGE).await?;
    let slug = access.tenant.slug.as_str();
    let settings: Vec<(String, String)> = TUNING_FIELDS
        .iter()
        .filter_map(|(field, _)| Some(((*field).to_owned(), form.get(*field)?.replace(',', "."))))
        .collect();
    match google(cx)
        .set_tuning(&access.user_id, access.tenant_id(), settings)
        .await
    {
        Ok(()) => Ok(see_other(tuning_url(
            cx,
            slug,
            None,
            Some("Thresholds saved."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(tuning_url(cx, slug, err.code(), None))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
