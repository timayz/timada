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
use crate::app::auth::home_url_with_message;
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
        Ok(joined) => {
            invite_flow(cx).remove();
            let tenant = joined.tenant;
            if joined.seat_pending {
                // The invitation is the account's; the tenant opens once the
                // seat lands, so wait for it at home.
                return Ok(see_other(home_url_with_message(
                    cx,
                    &format!(
                        "You joined {}. Your access arrives in a moment.",
                        tenant.name
                    ),
                )));
            }
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
