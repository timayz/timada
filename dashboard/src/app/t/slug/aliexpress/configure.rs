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
    app_key: String,
    #[serde(default)]
    app_secret: String,
    ship_to_country: String,
    currency: String,
    language: String,
    #[serde(flatten)]
    pricing: PricingForm,
}

/// Saves the shop's AliExpress app. A blank secret keeps the saved one, as
/// long as the key is the same.
#[route(POST)]
pub async fn configure(cx: &Cx, Form(form): Form<ConfigureForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let refused = |code: &str| Ok(see_other(aliexpress_url(cx, slug, Some(code), None)));
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    if !aliexpress.enabled() {
        return refused("secrets_disabled");
    }
    let pricing = match form.pricing.rule() {
        Ok(rule) => rule,
        Err(code) => return refused(code),
    };
    let app_key = form.app_key.trim().to_owned();
    let secret = form.app_secret.trim();
    let app_secret_sealed = if !secret.is_empty() {
        aliexpress
            .seal(secret)
            .map_err(topcoat::Error::from_anyhow)?
    } else {
        match &access.tenant.aliexpress {
            Some(saved) if saved.app_key == app_key => saved.app_secret_sealed.clone(),
            _ => return refused("aliexpress_secret_required"),
        }
    };
    let configured = tenancy(cx)
        .configure_aliexpress(
            &access.user_id,
            &access.tenant.id,
            ConfigureAliexpress {
                app_key,
                app_secret_sealed,
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
            Some("AliExpress app saved."),
        ))),
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(err.into()),
    }
}
