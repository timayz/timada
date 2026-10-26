use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::aliexpress_url;
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
    if access
        .tenant
        .aliexpress
        .as_ref()
        .is_none_or(|settings| settings.authorization.is_none())
    {
        return refused("aliexpress_not_connected");
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
