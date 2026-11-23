use timada_catalog::published_child_count;
use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{act, category_id, category_url};
use crate::app::t::slug::require_tenant;
use crate::db::Db;

#[derive(serde::Deserialize)]
pub struct PublishForm {
    /// `yes` shows the category to shoppers; anything else keeps it back.
    #[serde(default)]
    published: String,
}

/// Shows the category to shoppers, or keeps it back as a draft. One with
/// sub-categories shown to shoppers is not kept back from here, as one
/// with live sub-categories is not archived: the tree is the read model's.
#[route(POST)]
pub async fn publish(cx: &Cx, Form(form): Form<PublishForm>) -> Result<SeeOther> {
    let published = form.published == "yes";
    if !published {
        let access = require_tenant(cx).await?;
        access
            .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
            .await?;
        let category_id = category_id(cx);
        let below = published_child_count(context::<Db>(cx), &access.tenant.id, category_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        if below > 0 {
            return Ok(see_other(category_url(
                cx,
                &access.tenant.slug,
                category_id,
                Some("category_has_published_children"),
                None,
            )));
        }
    }
    let message = if published {
        "Category published."
    } else {
        "Category kept back as a draft."
    };
    act(cx, message, async |catalog, actor, id| {
        catalog.set_category_published(actor, id, published).await
    })
    .await
}
