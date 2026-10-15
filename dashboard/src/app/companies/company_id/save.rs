use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{company_url, require_owned};
use crate::app::companies::IdentityForm;
use crate::app::tenants::{error_code, tenancy};

/// Replaces the company's identity; its SIREN stays.
#[route(POST)]
pub async fn save(cx: &Cx, Form(form): Form<IdentityForm>) -> Result<SeeOther> {
    let (company, user_id) = require_owned(cx).await?;
    let identity = match form.identity() {
        Ok(identity) => identity,
        Err(code) => return Ok(see_other(company_url(cx, &company.id, Some(code), None))),
    };
    match tenancy(cx)
        .set_company_identity(&user_id, &company.id, identity)
        .await
    {
        Ok(()) => Ok(see_other(company_url(
            cx,
            &company.id,
            None,
            Some("Legal identity saved."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(company_url(
            cx,
            &company.id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
