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

use super::{gateway, require_drafter, writer_url};

#[derive(Deserialize)]
pub struct BriefForm {
    #[serde(default)]
    tone: String,
    #[serde(default)]
    never: String,
}

/// Says how the shop wants to be written for.
#[route(POST)]
pub async fn brief(cx: &Cx, Form(form): Form<BriefForm>) -> Result<SeeOther> {
    let access = require_drafter(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match gateway(&access)?
        .set_brief(&access.user_id, &form.tone, &form.never)
        .await
    {
        Ok(()) => Ok(see_other(writer_url(cx, slug, None, Some("Saved.")))),
        Err(err) if err.is_user_facing() => Ok(see_other(writer_url(cx, slug, err.code(), None))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
