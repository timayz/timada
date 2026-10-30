use std::collections::HashMap;
use std::sync::Arc;

use timada_tenancy::{ConfigureAliexpress, PERMISSION_ALIEXPRESS_MANAGE};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{aliexpress_url, pricing_of_form};
use crate::aliexpress::Aliexpress;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Saves what the shop buys on AliExpress and how it prices it. The form
/// is read as a map: its bands are numbered fields (see
/// [`pricing_of_form`]).
#[route(POST)]
pub async fn configure(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let refused = |code: &str| Ok(see_other(aliexpress_url(cx, slug, Some(code), None)));
    if !app_context::<Arc<Aliexpress>>(cx).enabled() {
        return refused("aliexpress_disabled");
    }
    let pricing = match pricing_of_form(&form) {
        Ok(pricing) => pricing,
        Err(code) => return refused(code),
    };
    let sent = |name: &str| form.get(name).cloned().unwrap_or_default();
    let configured = tenancy(cx)
        .configure_aliexpress(
            &access.user_id,
            &access.tenant.id,
            ConfigureAliexpress {
                ship_to_country: sent("ship_to_country"),
                currency: sent("currency"),
                language: sent("language"),
                pricing,
            },
        )
        .await;
    match configured {
        Ok(()) => Ok(see_other(aliexpress_url(
            cx,
            slug,
            None,
            Some("AliExpress settings saved."),
        ))),
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(err.into()),
    }
}
