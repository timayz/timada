use serde::Deserialize;
use timada_tenancy::{PERMISSION_MAIL_MANAGE, PickMailServers, TenantState};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::t::slug::mail::mail_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// The choice of the platform's server for the shop's mails.
pub const PLATFORM: &str = "platform";
/// The choice of no server for marketing.
pub const NONE: &str = "none";

#[derive(Deserialize)]
pub struct PickForm {
    #[serde(default)]
    shop_mails: String,
    #[serde(default)]
    marketing: String,
}

/// The server id a select sent; `None` for `nothing` or a blank.
fn picked(value: &str, nothing: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != nothing).then(|| value.to_owned())
}

/// What the shop now sends through what, in words.
fn saved_message(tenant: &TenantState, cmd: &PickMailServers) -> String {
    let label = |id: &Option<String>| {
        id.as_deref()
            .and_then(|id| tenant.mail_server(id))
            .map(|server| server.label().to_owned())
    };
    let shop_mails = label(&cmd.shop_mails).unwrap_or_else(|| "the platform's server".to_owned());
    let marketing = match label(&cmd.marketing) {
        Some(server) => format!("and its marketing through {server}"),
        None => "and no marketing goes out".to_owned(),
    };
    format!("Saved: the shop's mails go through {shop_mails}, {marketing}.")
}

/// Picks which server sends the shop's mails and which its marketing.
#[route(POST)]
pub async fn pick(cx: &Cx, Form(form): Form<PickForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let cmd = PickMailServers {
        shop_mails: picked(&form.shop_mails, PLATFORM),
        marketing: picked(&form.marketing, NONE),
    };
    let message = saved_message(&access.tenant, &cmd);
    match tenancy(cx)
        .pick_mail_servers(&access.user_id, &access.tenant.id, cmd)
        .await
    {
        Ok(()) => Ok(see_other(mail_url(cx, slug, None, Some(&message)))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(mail_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(err.into()),
    }
}
