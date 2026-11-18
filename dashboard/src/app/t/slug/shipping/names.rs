use std::collections::HashMap;

use serde::Deserialize;
use timada_orders::OrderError;
use timada_tenancy::PERMISSION_SHIPPING_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{error_code, orders, shipping_url};
use crate::app::t::slug::require_tenant;

/// What the fields of a language's names are named after: followed by the
/// method's id.
pub const NAME_FIELD_PREFIX: &str = "name:";

#[derive(Deserialize)]
pub struct ShippingNamesForm {
    language: String,
    /// The names, among anything else posted.
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

/// Says what a language calls the shipping methods: each one posted, an
/// empty name going back to the main language's.
#[route(POST)]
pub async fn names(cx: &Cx, Form(form): Form<ShippingNamesForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SHIPPING_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let gateway = orders(&access)?;
    let named = async {
        for (field, name) in &form.fields {
            if let Some(method_id) = field.strip_prefix(NAME_FIELD_PREFIX) {
                gateway
                    .name_shipping_method(&access.user_id, &form.language, method_id, name)
                    .await?;
            }
        }
        Ok::<(), OrderError>(())
    };
    match named.await {
        Ok(()) => Ok(see_other(shipping_url(
            cx,
            slug,
            None,
            Some("Names saved."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(shipping_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
