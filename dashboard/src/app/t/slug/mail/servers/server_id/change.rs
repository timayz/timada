use timada_tenancy::{PERMISSION_MAIL_MANAGE, TenancyError};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{ServerId, server_url};
use crate::app::t::slug::mail::mail_url;
use crate::app::t::slug::mail::servers::{ServerForm, tested};
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Saves a server's new settings once a test mail went out through them.
#[route(POST)]
pub async fn change(cx: &Cx, Form(form): Form<ServerForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let server_id = path_param::<ServerId>(cx).to_owned();
    let gone = || {
        Ok(see_other(mail_url(
            cx,
            slug,
            Some("mail_server_not_found"),
            None,
        )))
    };
    let Some(saved) = access.tenant.mail_server(&server_id) else {
        return gone();
    };
    let fail = |code: &str| {
        Ok(see_other(server_url(
            cx,
            slug,
            &server_id,
            Some(code),
            None,
        )))
    };
    let cmd = match tested(cx, &access.tenant, form, Some(saved)).await? {
        Ok(cmd) => cmd,
        Err(code) => return fail(code),
    };
    match tenancy(cx)
        .change_mail_server(&access.user_id, &access.tenant.id, &server_id, cmd)
        .await
    {
        Ok(()) => Ok(see_other(mail_url(
            cx,
            slug,
            None,
            Some("Test mail sent: server saved."),
        ))),
        Err(TenancyError::MailServerNotFound) => gone(),
        Err(err) if err.is_user_facing() => fail(error_code(&err)),
        Err(err) => Err(err.into()),
    }
}
