use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{ProductId, product_url};
use crate::app::t::slug::products::{catalog, error_code};
use crate::app::t::slug::require_tenant;
use crate::db::Db;

/// Retires the product for good. One imported from AliExpress is no longer
/// kept in step with it, so the item can be imported again.
#[route(POST)]
pub async fn archive(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let product_id = path_param::<ProductId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = catalog(&access)?;
    match gateway.archive_product(&access.user_id, &product_id).await {
        Ok(()) => {}
        Err(err) if err.is_user_facing() => {
            return Ok(see_other(product_url(
                cx,
                &slug,
                &product_id,
                Some(error_code(&err)),
                None,
            )));
        }
        Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
    }
    let db = app_context::<Db>(cx);
    let link = timada_aliexpress::link_of_product(db, &access.tenant.id, &product_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if let Some(link) = link {
        let links = timada_aliexpress::Command::new(access.executor()?.clone(), access.tenant_id());
        if let Err(err) = links.unlink(&access.user_id, &link.id).await {
            tracing::warn!(product = %product_id, link = %link.id, error = %err, "archived product still linked to AliExpress");
        }
    }
    Ok(see_other(product_url(
        cx,
        &slug,
        &product_id,
        None,
        Some("Product archived."),
    )))
}
