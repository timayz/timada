use serde::Deserialize;
use timada_catalog::CreateProduct;
use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::product_id::product_url;
use super::{catalog, error_code, products_url};
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
struct CreateForm {
    name: String,
    #[serde(default)]
    slug: Option<String>,
}

/// Creates a draft product and lands on its page.
#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<CreateForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let cmd = CreateProduct {
        name: form.name,
        slug: form.slug,
    };
    match catalog(&access)?.create_product(&access.user_id, cmd).await {
        Ok(product) => Ok(see_other(product_url(
            cx,
            slug,
            &product.id,
            None,
            Some("Product created."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(products_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
