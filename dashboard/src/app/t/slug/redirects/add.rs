use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{RedirectForm, redirects_url};
use crate::app::t::slug::announcements::{error_code, pages};
use crate::app::t::slug::require_tenant;

/// Adds a redirect and lands back on the list.
#[route(POST)]
pub async fn add(cx: &Cx, Form(form): Form<RedirectForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let added = pages(&access)?
        .add_redirect(&access.user_id, &form.from, &form.to)
        .await;
    match added {
        Ok(_) => Ok(see_other(redirects_url(
            cx,
            slug,
            None,
            Some("Redirect added: the storefront follows it in a moment."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(redirects_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
