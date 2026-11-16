//! `POST /t/{slug}/categories/filters` (`options=…`, one a line): the
//! options the shop's listings are narrowed by.

use serde::Deserialize;
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

use super::{categories_url, error_code};
use crate::app::t::slug::products::catalog;
use crate::app::t::slug::require_tenant;

#[derive(Deserialize)]
struct FiltersForm {
    #[serde(default)]
    options: String,
}

#[route(POST)]
pub async fn filters(cx: &Cx, Form(form): Form<FiltersForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let names: Vec<String> = form.options.lines().map(str::to_owned).collect();
    match catalog(&access)?
        .set_listing_filters(&access.user_id, &names)
        .await
    {
        Ok(()) => Ok(see_other(categories_url(
            cx,
            slug,
            None,
            Some("Filters saved."),
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
