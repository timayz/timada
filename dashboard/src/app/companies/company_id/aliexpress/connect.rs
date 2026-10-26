use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use crate::aliexpress::Aliexpress;
use crate::app::aliexpress::callback_url;
use crate::app::companies::company_id::{company_url, require_owned};
use crate::auth::cookies::{AliexpressFlow, aliexpress_flow};

/// Sends the owner to AliExpress, where the company's account says yes to
/// the platform's app; AliExpress sends them back to the callback with a
/// code.
#[route(POST)]
pub async fn connect(cx: &Cx) -> Result<SeeOther> {
    let (company, user_id) = require_owned(cx).await?;
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    if !aliexpress.enabled() {
        return Ok(see_other(company_url(
            cx,
            &company.id,
            Some("aliexpress_disabled"),
            None,
        )));
    }
    let state = format!("{:032x}", rand::random::<u128>());
    aliexpress_flow(cx)
        .set(AliexpressFlow {
            company_id: company.id.clone(),
            user_id,
            state: state.clone(),
            issued_at: crate::auth::now(),
        })
        .commit()?;
    let url = aliexpress
        .authorize_url(&callback_url(cx), &state)
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(see_other(url))
}
