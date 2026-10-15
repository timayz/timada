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
use crate::app::tenants::{error_code, tenancy};

#[derive(serde::Deserialize)]
pub struct RemoveOwnerForm {
    user_id: String,
}

/// Removes an owner; never the last one.
#[route(POST)]
pub async fn remove_owner(cx: &Cx, Form(form): Form<RemoveOwnerForm>) -> Result<SeeOther> {
    let (company, user_id) = require_owned(cx).await?;
    match tenancy(cx)
        .remove_company_owner(&user_id, &company.id, &form.user_id)
        .await
    {
        // Removing oneself leaves the page to the others.
        Ok(()) if form.user_id == user_id => Ok(see_other(crate::app::companies::companies_url(
            cx,
            None,
            Some("You no longer own the company."),
        ))),
        Ok(()) => Ok(see_other(company_url(
            cx,
            &company.id,
            None,
            Some("Owner removed."),
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
