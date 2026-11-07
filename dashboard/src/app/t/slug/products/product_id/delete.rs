//! `POST /t/{slug}/products/{product_id}/delete`: deletes a product nobody
//! ever ordered, for good. One that was ordered stays on record (archive it
//! instead): the catalog refuses it, from the mark the orders leave on it.

use std::sync::Arc;

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
use crate::app::t::slug::products::{catalog, error_code, products_url};
use crate::app::t::slug::require_tenant;
use crate::db::Db;
use crate::media::Media;

#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let product_id = path_param::<ProductId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = catalog(&access)?;
    let product = match gateway.delete_product(&access.user_id, &product_id).await {
        Ok(product) => product,
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
    };
    // This node's copies of its pictures go now; the other nodes' go when
    // the event reaches them.
    let media = app_context::<Arc<Media>>(cx);
    for image in &product.images {
        media.remove_image(&access.tenant.id, &image.id).await;
    }
    // Like an archived product, a deleted one is no longer kept in step
    // with the AliExpress item it came from, so the item can be imported
    // again.
    let db = app_context::<Db>(cx);
    let link = timada_aliexpress::link_of_product(db, &access.tenant.id, &product_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if let Some(link) = link {
        let links = timada_aliexpress::Command::new(access.executor()?.clone(), access.tenant_id());
        if let Err(err) = links.unlink(&access.user_id, &link.id).await {
            tracing::warn!(product = %product_id, link = %link.id, error = %err, "deleted product still linked to AliExpress");
        }
    }
    Ok(see_other(products_url(
        cx,
        &slug,
        None,
        Some("Product deleted; the list updates in a moment."),
    )))
}
