use timada_tenancy::PERMISSION_API_KEYS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::api_keys_url;
use crate::app::t::slug::orders::orders;
use crate::app::t::slug::require_tenant;

#[derive(serde::Deserialize)]
pub struct StorefrontForm {
    /// Blank to forget it.
    url: String,
}

/// Says where the tenant's storefront lives, so the order confirmations
/// mailed to its shoppers link to their orders there.
#[route(POST)]
pub async fn storefront(cx: &Cx, Form(form): Form<StorefrontForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_API_KEYS_MANAGE)
        .await?;
    let slug = access.tenant.slug.clone();
    let url = Some(form.url.trim()).filter(|url| !url.is_empty());
    match orders(&access)?
        .set_storefront_address(&access.user_id, url)
        .await
    {
        Ok(()) => {
            let message = if url.is_some() {
                "Storefront address saved."
            } else {
                "Storefront address removed."
            };
            Ok(see_other(api_keys_url(cx, &slug, None, Some(message))))
        }
        Err(err) => match err.code() {
            Some(code) => Ok(see_other(api_keys_url(cx, &slug, Some(code), None))),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
