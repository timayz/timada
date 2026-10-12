use timada_tenancy::AcceptInvitation;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{Token, invitation_url};
use crate::app::t::slug::tenant_url;
use crate::app::tenants::{error_code, tenancy};
use crate::auth::{cookies::invite_flow, now, require_user};

/// Joins the tenant the link belongs to, as the signed-in user.
#[route(POST)]
pub async fn accept(cx: &Cx) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    let token = path_param::<Token>(cx);
    match tenancy(cx)
        .accept_invitation(
            AcceptInvitation {
                token: token.to_owned(),
                user_id: user.id.clone(),
            },
            now(),
        )
        .await
    {
        Ok(tenant) => {
            invite_flow(cx).remove();
            Ok(see_other(tenant_url(
                cx,
                &tenant.slug,
                None,
                Some(&format!("You joined {}.", tenant.name)),
            )))
        }
        Err(err) if err.is_user_facing() => {
            Ok(see_other(invitation_url(cx, token, Some(error_code(&err)))))
        }
        Err(err) => Err(err.into()),
    }
}
