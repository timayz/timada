use timada_tenancy::PERMISSION_MAIL_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::ServerId;
use crate::app::t::slug::mail::mail_url;
use crate::app::t::slug::mail::servers::picked_for;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Deletes a server. What it was picked for has no server any more.
#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let server_id = path_param::<ServerId>(cx).to_owned();
    let (shop_mails, marketing) = picked_for(&access.tenant, &server_id);
    match tenancy(cx)
        .delete_mail_server(&access.user_id, &access.tenant.id, &server_id)
        .await
    {
        Ok(()) => {
            let mut message = "Server deleted.".to_owned();
            if shop_mails {
                message.push_str(" The shop's mails go out through the platform's server.");
            }
            if marketing {
                message.push_str(" Marketing waits until you pick another server for it.");
            }
            Ok(see_other(mail_url(cx, slug, None, Some(&message))))
        }
        Err(err) if err.is_user_facing() => {
            Ok(see_other(mail_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
