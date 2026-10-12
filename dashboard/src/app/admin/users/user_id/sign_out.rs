use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{UserId, user_url};
use crate::app::admin::{error_code, require_admin};
use crate::auth::identity;

/// Voids every session of the user, this browser's included if it is theirs.
#[route(POST)]
pub async fn sign_out(cx: &Cx) -> Result<SeeOther> {
    let me = require_admin(cx).await?;
    let id = path_param::<UserId>(cx);
    match identity(cx).revoke_sessions(&me.id, id, None).await {
        Ok(()) => Ok(see_other(user_url(
            cx,
            id,
            None,
            Some("Signed out everywhere."),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(user_url(cx, id, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
