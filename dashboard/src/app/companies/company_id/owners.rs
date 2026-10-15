use timada_identity::normalize_email;
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
use crate::auth::identity;

#[derive(serde::Deserialize)]
pub struct OwnerForm {
    email: String,
}

/// Makes a registered user, found by e-mail, an owner too.
#[route(POST)]
pub async fn add_owner(cx: &Cx, Form(form): Form<OwnerForm>) -> Result<SeeOther> {
    let (company, user_id) = require_owned(cx).await?;
    let account = match normalize_email(&form.email) {
        Ok(email) => identity(cx)
            .load_email_account(&email)
            .await
            .map_err(topcoat::Error::from_anyhow)?,
        Err(_) => None,
    };
    let Some(account) = account else {
        return Ok(see_other(company_url(
            cx,
            &company.id,
            Some("unknown_user"),
            None,
        )));
    };
    match tenancy(cx)
        .add_company_owner(&user_id, &company.id, &account.user_id)
        .await
    {
        Ok(()) => Ok(see_other(company_url(
            cx,
            &company.id,
            None,
            Some("Owner added."),
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
