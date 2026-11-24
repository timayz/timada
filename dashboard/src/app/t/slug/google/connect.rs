use serde::Deserialize;
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

use super::{google, google_url};
use crate::app::google::callback_url;
use crate::app::t::slug::require_tenant;
use crate::auth::cookies::{GoogleFlow, google_flow};
use crate::google::{Google, OFFERED};

/// The kinds of access ticked; a box left unticked posts nothing.
#[derive(Deserialize)]
pub struct ConnectForm {
    #[serde(default)]
    search_console: String,
    #[serde(default)]
    ads: String,
    #[serde(default)]
    merchant_center: String,
}

impl ConnectForm {
    fn ticked(&self, access: &str) -> bool {
        let value = match access {
            timada_google::ACCESS_SEARCH_CONSOLE => &self.search_console,
            timada_google::ACCESS_ADS => &self.ads,
            timada_google::ACCESS_MERCHANT_CENTER => &self.merchant_center,
            _ => return false,
        };
        !value.is_empty()
    }
}

/// Sends the member to Google, where the shop's account says yes to the
/// kinds of access ticked, and to the ones the connection covers already:
/// asking for one more never drops another. Google sends them back to the
/// callback with a code.
#[route(POST)]
pub async fn connect(cx: &Cx, Form(form): Form<ConnectForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let refused = |code: &str| Ok(see_other(google_url(cx, slug, Some(code), None)));
    let service = context::<Google>(cx);
    if !service.enabled() {
        return refused("google_disabled");
    }
    let connection = google(cx)
        .load_connection(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let accesses: Vec<String> = timada_google::ACCESSES
        .into_iter()
        .filter(|kind| {
            (OFFERED.contains(kind) && form.ticked(kind))
                || connection
                    .as_ref()
                    .is_some_and(|connection| connection.covers(kind))
        })
        .map(str::to_owned)
        .collect();
    if accesses.is_empty() {
        return refused("google_no_access_chosen");
    }
    let state = format!("{:032x}", rand::random::<u128>());
    google_flow(cx)
        .set(GoogleFlow {
            slug: slug.to_owned(),
            tenant_id: access.tenant_id().to_owned(),
            user_id: access.user_id.clone(),
            state: state.clone(),
            issued_at: crate::auth::now(),
        })
        .commit()?;
    let url = service
        .authorize_url(&callback_url(cx), &state, &accesses)
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(see_other(url))
}
