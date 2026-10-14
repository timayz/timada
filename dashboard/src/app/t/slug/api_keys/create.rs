use std::collections::HashMap;

use timada_tenancy::CreateApiKey;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{api_keys_url, checked_scopes};
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};
use crate::auth::cookies::{ApiKeyReveal, api_key_reveal};
use crate::auth::now;

/// Creates a key from the form (`label` and one `scope:…` field per ticked
/// scope) and hands its token to the keys page through a private
/// cookie, so it is shown once and never sits in a URL.
#[route(POST)]
pub async fn create(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    let slug = access.tenant.slug.as_str();
    let cmd = CreateApiKey {
        tenant_id: access.tenant.id.clone(),
        label: form.get("label").cloned().unwrap_or_default(),
        scopes: checked_scopes(&form),
    };
    match tenancy(cx)
        .create_api_key(&access.user_id, cmd, now())
        .await
    {
        Ok(created) => {
            api_key_reveal(cx)
                .set(ApiKeyReveal {
                    key_id: created.key.id,
                    label: created.key.label,
                    token: created.token,
                    issued_at: now(),
                })
                .commit()?;
            Ok(see_other(api_keys_url(
                cx,
                slug,
                None,
                Some("Key created."),
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
