use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{UserId, user_url};
use crate::app::admin::{error_code, require_admin};
use crate::auth::identity;

#[derive(Deserialize)]
struct RoleForm {
    admin: bool,
}

#[route(POST)]
pub async fn role(cx: &Cx, Form(form): Form<RoleForm>) -> Result<SeeOther> {
    let me = require_admin(cx).await?;
    let id = path_param::<UserId>(cx);
    match identity(cx).set_admin(&me.id, id, form.admin).await {
        Ok(()) => Ok(see_other(user_url(
            cx,
            id,
            None,
            Some(if form.admin {
                "Administrator role granted."
            } else {
                "Administrator role revoked."
            }),
        ))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(user_url(cx, id, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
