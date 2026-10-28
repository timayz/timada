use timada_tenancy::PERMISSION_AUTOMATIONS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{automations, automations_url};
use crate::app::t::slug::require_tenant;

#[derive(serde::Deserialize)]
pub struct AddressForm {
    #[serde(default)]
    email: String,
}

/// Puts an address on the shop's opt-out list, at the customer's request: it gets no automated mail any more.
#[route(POST)]
pub async fn opt_out(cx: &Cx, Form(form): Form<AddressForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_AUTOMATIONS_MANAGE)
        .await?;
    let slug = access.tenant.slug.clone();
    let done = automations(&access)?
        .opt_out_address(&access.user_id, &form.email)
        .await;
    match done {
        Ok(()) => Ok(see_other(automations_url(
            cx,
            &slug,
            None,
            Some("Done: this address gets no automated mail from the shop any more."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(automations_url(cx, &slug, err.code(), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
