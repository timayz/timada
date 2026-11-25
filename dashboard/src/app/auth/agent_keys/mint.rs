use std::collections::{BTreeMap, HashMap};

use timada_tenancy::MintAgentKey;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, not_found, see_other},
        route,
    },
};

use super::{GRANT_PREFIX, agent_keys_url};
use crate::app::tenants::{error_code, tenancy};
use crate::auth::cookies::{ApiKeyReveal, agent_key_reveal};
use crate::auth::{now, require_user};
use crate::config::Config;

/// Makes a key from the form (`label`, and one `grant:{shop}:{permission}`
/// field a ticked box) and hands its token to the keys page through a
/// private cookie, so it is shown once and never sits in a URL.
#[route(POST)]
pub async fn mint(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    if !context::<Config>(cx).mcp.enabled {
        return Err(not_found().into());
    }
    let mut shops: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in form.keys() {
        if let Some((shop, permission)) = name
            .strip_prefix(GRANT_PREFIX)
            .and_then(|grant| grant.split_once(':'))
        {
            shops
                .entry(shop.to_owned())
                .or_default()
                .push(permission.to_owned());
        }
    }
    let cmd = MintAgentKey {
        label: form.get("label").cloned().unwrap_or_default(),
        shops: shops.into_iter().collect(),
    };
    match tenancy(cx).mint_agent_key(&user.id, cmd).await {
        Ok(minted) => {
            agent_key_reveal(cx)
                .set(ApiKeyReveal {
                    key_id: minted.key.id,
                    label: minted.key.label,
                    token: minted.token,
                    issued_at: now(),
                })
                .commit()?;
            Ok(see_other(agent_keys_url(cx, None, Some("Key made."))))
        }
        Err(err) if err.is_user_facing() => {
            Ok(see_other(agent_keys_url(cx, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
