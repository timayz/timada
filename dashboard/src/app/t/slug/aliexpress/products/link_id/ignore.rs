use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{CombinationForm, back_to, link_id};
use crate::app::t::slug::require_tenant;

/// Stops offering a combination a read found.
#[route(POST)]
pub async fn ignore(cx: &Cx, Form(form): Form<CombinationForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link_id = link_id(cx).to_owned();
    let links = timada_aliexpress::Command::new(access.executor()?.clone(), access.tenant_id());
    match links
        .ignore_combination(&access.user_id, &link_id, form.sku_id.trim())
        .await
    {
        Ok(()) => Ok(back_to(
            cx,
            &access,
            &form.back,
            None,
            Some("No longer offered."),
        )),
        Err(err) => match err.code() {
            Some(code) => Ok(back_to(cx, &access, &form.back, Some(code), None)),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
