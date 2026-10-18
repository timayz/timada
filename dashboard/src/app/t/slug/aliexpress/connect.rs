use std::sync::Arc;

use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::{aliexpress_url, callback_url};
use crate::aliexpress::Aliexpress;
use crate::app::t::slug::require_tenant;
use crate::auth::cookies::{AliexpressFlow, aliexpress_flow};

/// Sends the merchant to AliExpress, where their account says yes to the
/// shop's app; AliExpress sends them back to the callback with a code.
#[route(POST)]
pub async fn connect(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let Some(settings) = &access.tenant.aliexpress else {
        return Ok(see_other(aliexpress_url(
            cx,
            slug,
            Some("aliexpress_not_configured"),
            None,
        )));
    };
    let state = format!("{:032x}", rand::random::<u128>());
    aliexpress_flow(cx)
        .set(AliexpressFlow {
            tenant_id: access.tenant.id.clone(),
            user_id: access.user_id.clone(),
            state: state.clone(),
            issued_at: crate::auth::now(),
        })
        .commit()?;
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    Ok(see_other(aliexpress.authorize_url(
        &settings.app_key,
        &callback_url(cx, slug),
        &state,
    )))
}
