use serde::Deserialize;
use timada_google::ACCESS_ADS;
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

use super::{ads_accounts_of, error_code, fault_code, google, google_url};
use crate::app::t::slug::require_tenant;
use crate::google::Google;

#[derive(Deserialize)]
pub struct AdsAccountForm {
    #[serde(default)]
    customer_id: String,
}

/// Says which of the Google Ads accounts the grant reaches is the shop's.
/// Only one it reaches is taken: the list is asked of Google again, which
/// also says the account's name, its currency and its time zone.
#[route(POST)]
pub async fn ads_account(cx: &Cx, Form(form): Form<AdsAccountForm>) -> Result<SeeOther> {
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
    if !connection.covers(ACCESS_ADS) {
        return refused("google_no_ads");
    }
    let accounts = match ads_accounts_of(cx, context::<Google>(cx), &connection).await {
        Ok(accounts) => accounts,
        Err(fault) => return refused(fault_code(&fault)),
    };
    let Some(account) = accounts
        .into_iter()
        .find(|known| known.customer_id == form.customer_id)
    else {
        return refused("google_unknown_ads_account");
    };
    match google(cx)
        .choose_ads_account(&access.user_id, access.tenant_id(), account)
        .await
    {
        Ok(()) => Ok(see_other(google_url(
            cx,
            slug,
            None,
            Some("Google Ads account saved."),
        ))),
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
