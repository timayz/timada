use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{CombinationForm, LinkId, back_to};
use crate::app::t::slug::require_tenant;

/// Offers an ignored combination again.
#[route(POST)]
pub async fn restore(cx: &Cx, Form(form): Form<CombinationForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link_id = path_param::<LinkId>(cx).to_owned();
    let links = timada_aliexpress::Command(access.executor()?.clone());
    match links
        .restore_combination(&access.user_id, &link_id, form.sku_id.trim())
        .await
    {
        Ok(()) => Ok(back_to(
            cx,
            &access,
            &form.back,
            None,
            Some("Offered again."),
        )),
        Err(err) => match err.code() {
            Some(code) => Ok(back_to(cx, &access, &form.back, Some(code), None)),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
