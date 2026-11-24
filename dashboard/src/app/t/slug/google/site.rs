use serde::Deserialize;
use timada_google::ACCESS_SEARCH_CONSOLE;
use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{error_code, fault_code, google, google_url, sites_of};
use crate::app::t::slug::require_tenant;
use crate::google::Google;

#[derive(Deserialize)]
pub struct SiteForm {
    #[serde(default)]
    site: String,
}

/// Says which of the account's Search Console sites is the shop's. Only one
/// the account reads is taken: the list is asked of Google again.
#[route(POST)]
pub async fn site(cx: &Cx, Form(form): Form<SiteForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let refused = |code: &str| Ok(see_other(google_url(cx, slug, Some(code), None)));
    let connection = google(cx)
        .load_connection(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let Some(connection) = connection.filter(|connection| connection.connected) else {
        return refused("google_not_connected");
    };
    if !connection.covers(ACCESS_SEARCH_CONSOLE) {
        return refused("google_no_search_console");
    }
    let sites = match sites_of(cx, context::<Google>(cx), &connection).await {
        Ok(sites) => sites,
        Err(fault) => return refused(fault_code(&fault)),
    };
    if !sites.iter().any(|known| known.site == form.site) {
        return refused("google_unknown_site");
    }
    match google(cx)
        .choose_site(&access.user_id, access.tenant_id(), &form.site)
        .await
    {
        Ok(()) => Ok(see_other(google_url(
            cx,
            slug,
            None,
            Some("Search Console site saved."),
        ))),
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
