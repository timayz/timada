//! `/t/{slug}/mail/templates/{kind}`: one of the shop's mails. Each language
//! the shop mails in has an editor of its own under it
//! ([`language`]); this address opens the default language's.

pub mod language;

use timada_tenancy::{PERMISSION_MAIL_MANAGE, ShopMailKind};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, not_found, see_other},
        module_param, path_param, route,
    },
};

use crate::app::t::slug::require_tenant;

module_param!(pub kind);

/// The mail named in the URL, or 404.
pub fn require_kind(cx: &Cx) -> Result<ShopMailKind> {
    ShopMailKind::from_key(path_param::<Kind>(cx)).ok_or_else(|| not_found().into())
}

/// Opens the mail's editor in the shop's default language.
#[route(GET)]
pub async fn index(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let kind = require_kind(cx)?;
    Ok(see_other(language::editor_url(
        cx,
        &access.tenant.slug,
        kind,
        access.tenant.mail_default_language(),
        None,
        None,
    )))
}
