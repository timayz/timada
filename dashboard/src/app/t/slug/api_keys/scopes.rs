use std::collections::HashMap;
use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{api_keys_url, checked_scopes};
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};
use crate::auth::now;
use crate::store_api::StoreConnections;

/// Replaces a key's scopes with the ones ticked (`key_id` and one `scope:…`
/// field per scope). Its live store connections are ended: a connection
/// keeps the scopes it was opened with, so the stores reconnect under the
/// new ones.
#[route(POST)]
pub async fn scopes(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    let key_id = form.get("key_id").cloned().unwrap_or_default();
    match tenancy(cx)
        .change_api_key_scopes(&access.user_id, &key_id, checked_scopes(&form), now())
        .await
    {
        Ok(()) => {
            let ended = app_context::<Arc<StoreConnections>>(cx).close_key(&key_id);
            if ended > 0 {
                tracing::info!(
                    tenant = slug,
                    connections = ended,
                    "store connections ended by a key scope change"
                );
            }
            Ok(see_other(api_keys_url(
                cx,
                slug,
                None,
                Some("Scopes saved."),
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
