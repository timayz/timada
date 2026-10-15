use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::admin_companies_url;
use crate::app::admin::require_admin;
use crate::app::tenants::{error_code, tenancy};

#[derive(serde::Deserialize)]
pub struct ReleaseForm {
    siren: String,
}

/// Frees a SIREN so its rightful company can register it.
#[route(POST)]
pub async fn release(cx: &Cx, Form(form): Form<ReleaseForm>) -> Result<SeeOther> {
    let admin = require_admin(cx).await?;
    match tenancy(cx)
        .release_siren(&admin.id, form.siren.trim())
        .await
    {
        Ok(()) => Ok(see_other(admin_companies_url(
            cx,
            None,
            Some("SIREN released."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(admin_companies_url(
            cx,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
