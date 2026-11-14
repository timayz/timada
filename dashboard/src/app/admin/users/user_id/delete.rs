use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::{user_id, user_url};
use crate::app::admin::users::users_url;
use crate::app::admin::{error_code, require_admin};
use crate::auth::identity;

#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<SeeOther> {
    let me = require_admin(cx).await?;
    let id = user_id(cx);
    match identity(cx).delete_user(&me.id, id).await {
        Ok(()) => Ok(see_other(users_url(cx, Some("Account deleted.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(user_url(cx, id, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
