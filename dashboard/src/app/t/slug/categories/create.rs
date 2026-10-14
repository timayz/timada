use serde::Deserialize;
use timada_catalog::CreateCategory;
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

use super::category_id::category_url;
use super::{categories_url, error_code};
use crate::app::t::slug::products::catalog;
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
struct CreateForm {
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
}

/// Creates a category and lands on its page.
#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<CreateForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let cmd = CreateCategory {
        name: form.name,
        slug: form.slug,
        parent_id: form.parent_id,
    };
    match catalog(&access)?
        .create_category(&access.user_id, cmd)
        .await
    {
        Ok(category) => Ok(see_other(category_url(
            cx,
            slug,
            &category.id,
            None,
            Some("Category created."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(categories_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
