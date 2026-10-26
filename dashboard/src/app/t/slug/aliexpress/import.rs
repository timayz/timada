use std::sync::Arc;

use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::aliexpress_url;
use crate::aliexpress::Aliexpress;
use crate::app::t::slug::require_tenant;

#[derive(serde::Deserialize)]
pub struct ImportForm {
    product: String,
}

/// Asks for an AliExpress product to be imported, by its number or address;
/// the dashboard fetches it in the background.
#[route(POST)]
pub async fn import(cx: &Cx, Form(form): Form<ImportForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let refused = |code: &str| Ok(see_other(aliexpress_url(cx, slug, Some(code), None)));
    if access.tenant.aliexpress.is_none() {
        return refused("aliexpress_not_configured");
    }
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    let company = aliexpress
        .company_of(&access.tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    match company {
        None => return refused("aliexpress_no_company"),
        Some(company) if company.aliexpress.is_none() => {
            return refused("aliexpress_not_connected");
        }
        Some(_) => {}
    }
    let Some(ae_product_id) = timada_aliexpress::parse_product_id(&form.product) else {
        return refused("invalid_aliexpress_product");
    };
    let links = timada_aliexpress::Command::new(access.executor()?.clone(), access.tenant_id());
    match links.request_import(&access.user_id, &ae_product_id).await {
        Ok(_) => Ok(see_other(aliexpress_url(
            cx,
            slug,
            None,
            Some("Import started: the product shows below in a moment."),
        ))),
        Err(err) => match err.code() {
            Some(code) => refused(code),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
