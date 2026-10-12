use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::api_keys_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};
use crate::auth::now;

#[derive(Deserialize)]
struct RevokeForm {
    key_id: String,
}

/// Revokes a key for good; a storefront presenting it is refused from now on.
#[route(POST)]
pub async fn revoke(cx: &Cx, Form(form): Form<RevokeForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .revoke_api_key(&access.user_id, &form.key_id, now())
        .await
    {
        Ok(()) => Ok(see_other(api_keys_url(
            cx,
            slug,
            None,
            Some("Key revoked."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(api_keys_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
