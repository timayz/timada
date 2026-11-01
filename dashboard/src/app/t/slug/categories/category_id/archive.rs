use timada_catalog::live_child_count;
use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{CategoryId, act, category_url};
use crate::app::t::slug::require_tenant;
use crate::db::Db;

/// Archives the category. One with live sub-categories is refused here
/// rather than in the domain: the tree is the back office's view of the
/// catalog, and the read model is where that view lives.
#[route(POST)]
pub async fn archive(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let category_id = path_param::<CategoryId>(cx);
    let children = live_child_count(app_context::<Db>(cx), &access.tenant.id, category_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if children > 0 {
        return Ok(see_other(category_url(
            cx,
            &access.tenant.slug,
            category_id,
            Some("category_has_children"),
            None,
        )));
    }
    act(cx, "Category archived.", async |catalog, actor, id| {
        catalog.archive_category(actor, id).await
    })
    .await
}
