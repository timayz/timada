use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{Back, LinkId, back_to};
use crate::app::t::slug::require_tenant;

/// Stops keeping the product in step with AliExpress; it stays as it is.
#[route(POST)]
pub async fn unlink(cx: &Cx, Form(form): Form<Back>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link_id = path_param::<LinkId>(cx).to_owned();
    let links = timada_aliexpress::Command::new(access.executor()?.clone(), access.tenant_id());
    match links.unlink(&access.user_id, &link_id).await {
        Ok(()) => Ok(back_to(
            cx,
            &access,
            &form.back,
            None,
            Some("No longer kept in step with AliExpress."),
        )),
        Err(err) => match err.code() {
            Some(code) => Ok(back_to(cx, &access, &form.back, Some(code), None)),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
