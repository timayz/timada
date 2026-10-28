//! `/t/{slug}/mail/servers/{server_id}`: one of the shop's mail servers,
//! with the form that changes it (a test mail first, as when it was added)
//! and the one that deletes it.

pub mod change;
pub mod delete;

use timada_tenancy::{MailServerSettings, PERMISSION_MAIL_MANAGE};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::not_found, href, module_param, page, path_param, query_params},
    view::{View, attributes, view},
};

use super::{picked_for, security_label, server_form};
use crate::app::auth::Flash;
use crate::app::t::slug::mail::{mail_url, own_address};
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::auth::require_user;
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::shop_mail::ShopMail;
use crate::ui::{Shell, document, fact, facts, format_time, notice, page_header, section_card};

module_param!(pub server_id);

#[topcoat::router::query_params(error = bad_request)]
struct ServerQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn server_url(
    cx: &Cx,
    slug: &str,
    server_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, Slug(slug.to_owned()), ServerId(server_id.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The server named in the URL, or 404.
fn require_server(cx: &Cx, access: &TenantAccess) -> Result<MailServerSettings> {
    let id = path_param::<ServerId>(cx);
    access
        .tenant
        .mail_server(id)
        .cloned()
        .ok_or_else(|| not_found().into())
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let query = query_params::<ServerQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let server = require_server(cx, &access)?;
    let user = require_user(cx).await?;
    let test_to = own_address(user);
    let enabled = app_context::<ShopMail>(cx).enabled();
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let label = server.label().to_owned();
    let title = format!("{label} · Mail · {}", tenant.name);
    let back = mail_url(cx, &slug, None, None);
    let change_action = href!(
        change::change,
        Slug(slug.clone()),
        ServerId(server.id.clone())
    )
    .resolve(cx);
    let (shop_mails, marketing) = picked_for(&tenant, &server.id);
    let sends = match (shop_mails, marketing) {
        (true, true) => "The shop's mails and its marketing",
        (true, false) => "The shop's mails",
        (false, true) => "The shop's marketing",
        (false, false) => "Nothing yet: pick it on the Mail page",
    };
    let after_delete = match (shop_mails, marketing) {
        (true, true) => {
            "The shop's mails then go out through the platform's server, and its marketing waits until you pick another server for it."
        }
        (true, false) => "The shop's mails then go out through the platform's server.",
        (false, true) => {
            "Newsletter campaigns and automated mails then wait until you pick another server for marketing."
        }
        (false, false) => "Nothing is picked to go through it.",
    };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "mail"),
            page_header(
                title: &label,
                subtitle: Some("One of the mail servers your shop can send through."),
                h1_attrs: attributes! { data-mail-server-name="" },
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"All mail settings"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Settings"),
                facts(
                    fact(term: "Server", <code>(&server.host) ":" (server.port)</code>)
                    fact(term: "Security", (security_label(&server.security)))
                    fact(term: "User name", (server.username.as_deref().unwrap_or("None")))
                    fact(term: "Password", (if server.password_sealed.is_some() { "Saved, never shown again" } else { "None" }))
                    fact(
                        term: "Sends from",
                        (server.from_name.as_deref().unwrap_or(&tenant.name))
                        " <" (&server.from_address) ">"
                    )
                    fact(term: "Sends", attrs: attributes! { data-mail-server-sends="" }, (sends))
                    fact(term: "Since", <time datetime=(server.configured_at)>(format_time(server.configured_at))</time>)
                )
            )

            if enabled {
                section_card(
                    title: Some("Change the server"),
                    description: Some("Saving sends a test mail through the new settings first: they are kept only if it goes out."),
                    server_form(action: change_action, current: &server, shop_name: &tenant.name, test_to: &test_to)
                )
            }

            section_card(
                title: Some("Delete this server"),
                description: Some(after_delete),
                <form method="post" action=(href!(delete::delete, Slug(slug.clone()), ServerId(server.id.clone())))>
                    button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete this server")
                </form>
            )
        )
    })
}
