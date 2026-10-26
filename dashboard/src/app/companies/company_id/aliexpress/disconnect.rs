use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::companies::company_id::{company_url, require_owned};
use crate::app::tenants::{error_code, tenancy};

/// Forgets the company's AliExpress account: its shops import nothing and
/// place no order there until an owner connects one again. What they
/// imported stays as it is.
#[route(POST)]
pub async fn disconnect(cx: &Cx) -> Result<SeeOther> {
    let (company, user_id) = require_owned(cx).await?;
    match tenancy(cx)
        .disconnect_aliexpress(&user_id, &company.id)
        .await
    {
        Ok(()) => Ok(see_other(company_url(
            cx,
            &company.id,
            None,
            Some("AliExpress account disconnected."),
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
