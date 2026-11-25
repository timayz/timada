use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, not_found, see_other},
        route,
    },
};

use super::agent_keys_url;
use crate::app::tenants::{error_code, tenancy};
use crate::auth::require_user;
use crate::config::Config;

#[derive(Deserialize)]
pub struct WithdrawForm {
    #[serde(default)]
    key_id: String,
}

/// Withdraws one of the user's keys: an assistant presenting its token is
/// turned away from the next call on.
#[route(POST)]
pub async fn withdraw(cx: &Cx, Form(form): Form<WithdrawForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    if !context::<Config>(cx).mcp.enabled {
        return Err(not_found().into());
    }
    match tenancy(cx).withdraw_agent_key(&user.id, &form.key_id).await {
        Ok(()) => Ok(see_other(agent_keys_url(cx, None, Some("Key withdrawn.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(agent_keys_url(cx, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
