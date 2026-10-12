use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
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
use crate::store_api::StoreConnections;

#[derive(Deserialize)]
struct RevokeForm {
    key_id: String,
}

/// Revokes a key for good; its live store connections are ended and a
/// storefront presenting it is refused from now on.
#[route(POST)]
pub async fn revoke(cx: &Cx, Form(form): Form<RevokeForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .revoke_api_key(&access.user_id, &form.key_id, now())
        .await
    {
        Ok(()) => {
            let ended = app_context::<Arc<StoreConnections>>(cx).close_key(&form.key_id);
            if ended > 0 {
                tracing::info!(
                    tenant = slug,
                    connections = ended,
                    "store connections ended by a key revocation"
                );
            }
            Ok(see_other(api_keys_url(
                cx,
                slug,
                None,
                Some("Key revoked."),
            )))
        }
        Err(err) if err.is_user_facing() => Ok(see_other(api_keys_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
