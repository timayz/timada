use std::sync::Arc;

use serde::Deserialize;
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

use super::{PricingForm, aliexpress_url};
use crate::aliexpress::Aliexpress;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

#[derive(Deserialize)]
pub struct ConfigureForm {
    ship_to_country: String,
    currency: String,
    language: String,
    #[serde(flatten)]
    pricing: PricingForm,
}

/// Saves what the shop buys on AliExpress and how it prices it.
#[route(POST)]
pub async fn configure(cx: &Cx, Form(form): Form<ConfigureForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let refused = |code: &str| Ok(see_other(aliexpress_url(cx, slug, Some(code), None)));
    if !app_context::<Arc<Aliexpress>>(cx).enabled() {
        return refused("aliexpress_disabled");
    }
    let pricing = match form.pricing.rule() {
        Ok(rule) => rule,
        Err(code) => return refused(code),
    };
    let configured = tenancy(cx)
        .configure_aliexpress(
            &access.user_id,
            &access.tenant.id,
            ConfigureAliexpress {
                ship_to_country: form.ship_to_country,
                currency: form.currency,
                language: form.language,
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
