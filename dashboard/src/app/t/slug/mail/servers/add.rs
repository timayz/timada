use timada_tenancy::{MAX_MAIL_SERVERS, PERMISSION_MAIL_MANAGE};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{ServerForm, tested};
use crate::app::t::slug::mail::mail_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Adds a mail server once a test mail went out through it. The shop's
/// first server sends both its mails and its marketing.
#[route(POST)]
pub async fn add(cx: &Cx, Form(form): Form<ServerForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let fail = |code: &str| Ok(see_other(mail_url(cx, slug, Some(code), None)));
    // Before the test mail, so a server refused anyway gets none.
    if access.tenant.mail_servers.len() >= MAX_MAIL_SERVERS {
        return fail("too_many_mail_servers");
    }
    let cmd = match tested(cx, &access.tenant, form, None).await? {
        Ok(cmd) => cmd,
        Err(code) => return fail(code),
    };
    let message = if access.tenant.mail_servers.is_empty() {
        "Test mail sent: your shop's mails and its marketing now go out through this server."
    } else {
        "Test mail sent: server added. Pick below what it sends."
    };
    match tenancy(cx)
        .add_mail_server(&access.user_id, &access.tenant.id, cmd)
        .await
    {
        Ok(_) => Ok(see_other(mail_url(cx, slug, None, Some(message)))),
        Err(err) if err.is_user_facing() => fail(error_code(&err)),
        Err(err) => Err(err.into()),
    }
}
