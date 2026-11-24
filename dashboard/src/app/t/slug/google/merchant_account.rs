use serde::Deserialize;
use timada_google::ACCESS_MERCHANT_CENTER;
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

use super::{error_code, fault_code, google, google_url, merchant_accounts_of};
use crate::app::t::slug::require_tenant;
use crate::google::Google;

#[derive(Deserialize)]
pub struct MerchantAccountForm {
    #[serde(default)]
    account_id: String,
}

/// Says which of the Merchant Center accounts the grant reaches is the
/// shop's. Only one it reaches is taken: the list is asked of Google again,
/// which also says the account's name.
#[route(POST)]
pub async fn merchant_account(cx: &Cx, Form(form): Form<MerchantAccountForm>) -> Result<SeeOther> {
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
    if !connection.covers(ACCESS_MERCHANT_CENTER) {
        return refused("google_no_merchant_center");
    }
    let accounts = match merchant_accounts_of(cx, context::<Google>(cx), &connection).await {
        Ok(accounts) => accounts,
        Err(fault) => return refused(fault_code(&fault)),
    };
    let Some(account) = accounts
        .into_iter()
        .find(|known| known.account_id == form.account_id)
    else {
        return refused("google_unknown_merchant_account");
    };
    match google(cx)
        .choose_merchant_account(&access.user_id, access.tenant_id(), account)
        .await
    {
        Ok(()) => Ok(see_other(google_url(
            cx,
            slug,
            None,
            Some("Merchant Center account saved."),
        ))),
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
