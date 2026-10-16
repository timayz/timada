//! `/t/{slug}/mail`: the mail server the shop's mails to its shoppers go
//! out through, for members with `mail:manage`. Without one they go out
//! through the platform's server, under the shop's name. Saving sends a test
//! mail through the new settings first: what cannot deliver is not kept.

pub mod branding;
pub mod configure;
pub mod remove;
pub mod templates;

use std::sync::Arc;

use timada_identity::UserState;
use timada_tenancy::{
    MAIL_SECURITY_NONE, MAIL_SECURITY_STARTTLS, MAIL_SECURITY_TLS, MAX_MAIL_FOOTER_LEN,
    PERMISSION_MAIL_MANAGE, ShopMailKind,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::app::auth::Flash;
use crate::auth::{require_user, user_labels};
use crate::media::Media;
use crate::shop_mail::ShopMail;
use crate::ui::{document, format_time, notice};

#[topcoat::router::query_params(error = bad_request)]
struct MailQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn mail_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// Where a member's test mails go unless they say otherwise: their own
/// address.
pub fn own_address(user: &UserState) -> String {
    user.password
        .as_ref()
        .map(|password| password.email.clone())
        .or_else(|| {
            user.oauth2_accounts
                .iter()
                .find_map(|account| account.email.clone())
        })
        .unwrap_or_default()
}

/// How a connection is secured, as the form names it.
fn security_label(security: &str) -> &'static str {
    match security {
        MAIL_SECURITY_TLS => "TLS",
        MAIL_SECURITY_STARTTLS => "STARTTLS",
        MAIL_SECURITY_NONE => "None",
        _ => "Unknown",
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let query = query_params::<MailQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let user = require_user(cx).await?;
    let test_to = own_address(user);
    let TenantAccess { tenant, .. } = access;
    let enabled = app_context::<ShopMail>(cx).enabled();
    let slug = tenant.slug.clone();
    let settings = tenant.mail_server.clone();
    let title = format!("Mail · {}", tenant.name);
    let current = settings.clone().unwrap_or_default();
    let port = settings
        .as_ref()
        .map_or_else(|| "587".to_owned(), |settings| settings.port.to_string());
    let security = if settings.is_some() {
        current.security.clone()
    } else {
        MAIL_SECURITY_STARTTLS.to_owned()
    };
    let has_password = current.password_sealed.is_some();
    let editors: Vec<String> = ShopMailKind::ALL
        .iter()
        .filter_map(|kind| tenant.mail_template(*kind))
        .map(|template| template.saved_by.clone())
        .collect();
    let labels = user_labels(cx, &editors).await?;
    let branding = tenant.mail_branding.clone().unwrap_or_default();
    let logo_url = branding
        .logo_file
        .as_deref()
        .map(|file| app_context::<Arc<Media>>(cx).url(&tenant.id, file));
    let max_footer = MAX_MAIL_FOOTER_LEN.to_string();
    let mails: Vec<(ShopMailKind, Option<(String, u64)>)> = ShopMailKind::ALL
        .into_iter()
        .map(|kind| {
            let edited = tenant.mail_template(kind).map(|template| {
                let by = labels
                    .get(&template.saved_by)
                    .cloned()
                    .unwrap_or_else(|| template.saved_by.clone());
                (by, template.saved_at)
            });
            (kind, edited)
        })
        .collect();

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Mail"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "mail")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if !enabled {
                <p role="alert" class="mb-4 rounded border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900">
                    "Shop mail servers are not enabled on this platform: the operator has to set them up first."
                </p>
            }

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Mail server"</h2>
                if let Some(settings) = &settings {
                    <dl class="mb-4 grid grid-cols-[12rem_1fr] gap-y-1 text-sm" data-mail-server="own">
                        <dt class="text-neutral-500">"Server"</dt>
                        <dd><code data-mail-host="">(&settings.host) ":" (settings.port)</code></dd>
                        <dt class="text-neutral-500">"Security"</dt><dd>(security_label(&settings.security))</dd>
                        <dt class="text-neutral-500">"User name"</dt>
                        <dd>(settings.username.as_deref().unwrap_or("None"))</dd>
                        <dt class="text-neutral-500">"Password"</dt>
                        <dd>(if settings.password_sealed.is_some() { "Saved, never shown again" } else { "None" })</dd>
                        <dt class="text-neutral-500">"Sent from"</dt>
                        <dd data-mail-from="">
                            (settings.from_name.as_deref().unwrap_or(&tenant.name))
                            " <" (&settings.from_address) ">"
                        </dd>
                        <dt class="text-neutral-500">"Since"</dt>
                        <dd><time datetime=(settings.configured_at)>(format_time(settings.configured_at))</time></dd>
                    </dl>
                    <form method="post" action=(href!(remove::remove, Slug(slug.clone())))>
                        <button class="rounded border border-red-300 px-3 py-1 text-sm text-red-800 hover:bg-red-50">"Use the platform's server"</button>
                    </form>
                } else {
                    <p class="mb-2 text-sm" data-mail-server="platform">
                        "Not set up: your order confirmations, review requests and sign-in codes go out through the platform's server, under the name "
                        <strong>(&tenant.name)</strong> "."
                    </p>
                }
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4" data-mail-templates="">
                <h2 class="mb-2 text-lg font-medium">"Mails to shoppers"</h2>
                <p class="mb-4 text-sm text-neutral-600">
                    "Word the mails your shop sends in your own way. A mail you have not changed goes out as below."
                </p>
                <ul class="divide-y divide-neutral-200">
                    for (kind, edited) in &mails {
                        <li class="flex items-center justify-between py-2" data-mail-template=(kind.key())>
                            <div>
                                <a class="font-medium underline"
                                    href=(href!(templates::kind::index, Slug(slug.clone()), templates::kind::Kind(kind.key().to_owned())))>
                                    (kind.label())
                                </a>
                                <p class="text-sm text-neutral-500">(kind.description())</p>
                            </div>
                            <span class="text-sm text-neutral-600">
                                if let Some((by, at)) = edited {
                                    "Edited by " (by) " on " <time datetime=(at)>(format_time(*at))</time>
                                } else {
                                    "Built-in wording"
                                }
                            </span>
                        </li>
                    }
                </ul>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4" data-mail-branding="">
                <h2 class="mb-2 text-lg font-medium">"Look of your mails"</h2>
                <p class="mb-4 text-sm text-neutral-600">
                    "A logo, an accent colour and a footer, shared by all the mails above. In your own wording, use "
                    <code>"{{ shop.logo_url }}"</code> ", " <code>"{{ shop.accent }}"</code> " and " <code>"{{ shop.footer }}"</code> "."
                </p>
                <form method="post" enctype="multipart/form-data" action=(href!(branding::save, Slug(slug.clone()))) class="space-y-3">
                    if let Some(logo_url) = &logo_url {
                        <img src=(logo_url) alt="Your logo" data-mail-logo="" class="max-h-16 rounded border border-neutral-200 p-1">
                        <label class="flex items-center gap-2 text-sm">
                            <input type="checkbox" name="remove_logo" value="1">
                            "Remove the logo"
                        </label>
                    }
                    <label class="block text-sm" for="mail-logo">
                        if logo_url.is_some() { "Replace the logo" } else { "Logo" }
                    </label>
                    <input id="mail-logo" name="logo" type="file" accept="image/png,image/jpeg,image/webp,image/gif" class="block text-sm">
                    <label class="block text-sm" for="mail-accent">"Accent colour"</label>
                    <input id="mail-accent" name="accent" maxlength="7" placeholder="#1a73e8" pattern="#[0-9a-fA-F]{6}"
                        value=(branding.accent.as_deref().unwrap_or_default())
                        class="w-40 rounded border border-neutral-300 px-3 py-2 font-mono">
                    <label class="block text-sm" for="mail-footer">"Footer"</label>
                    <textarea id="mail-footer" name="footer" rows="3" maxlength=(&max_footer)
                        placeholder="Your shop's address, a line about returns…"
                        class="w-full rounded border border-neutral-300 px-3 py-2">(branding.footer.as_deref().unwrap_or_default())</textarea>
                    <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Save the look"</button>
                </form>
                if tenant.mail_branding.is_some() {
                    <form method="post" class="mt-3" action=(href!(branding::clear::clear, Slug(slug.clone())))>
                        <button class="rounded border border-red-300 px-3 py-1 text-sm text-red-800 hover:bg-red-50">"Back to the plain look"</button>
                    </form>
                }
            </section>

            if enabled {
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">
                        if settings.is_some() { "Change the server" } else { "Send from your own server" }
                    </h2>
                    <p class="mb-4 text-sm text-neutral-600">
                        "Your mail provider gives these settings for sending (SMTP). Saving sends a test mail through them first: they are kept only if it goes out."
                    </p>
                    <form method="post" action=(href!(configure::configure, Slug(slug.clone()))) class="space-y-3">
                        <label class="block text-sm" for="mail-host">"Server"</label>
                        <input id="mail-host" name="host" required="" maxlength="253" placeholder="smtp.example.com" value=(&current.host)
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                        <label class="block text-sm" for="mail-port">"Port"</label>
                        <input id="mail-port" name="port" type="number" min="1" max="65535" required="" value=(&port)
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                        <label class="block text-sm" for="mail-security">"Security"</label>
                        <select id="mail-security" name="security" class="w-full rounded border border-neutral-300 px-3 py-2">
                            for (value, label) in [
                                (MAIL_SECURITY_STARTTLS, "STARTTLS (usually port 587)"),
                                (MAIL_SECURITY_TLS, "TLS (usually port 465)"),
                                (MAIL_SECURITY_NONE, "None (not encrypted)"),
                            ] {
                                <option value=(value) selected=(security == value)>(label)</option>
                            }
                        </select>
                        <label class="block text-sm" for="mail-username">"User name"</label>
                        <input id="mail-username" name="username" maxlength="254" autocomplete="off" value=(current.username.as_deref().unwrap_or_default())
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                        <label class="block text-sm" for="mail-password">"Password"</label>
                        <input id="mail-password" name="password" type="password" autocomplete="off"
                            placeholder=(if has_password { "Leave blank to keep the saved one" } else { "" })
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono">
                        <label class="block text-sm" for="mail-from-address">"Send from address"</label>
                        <input id="mail-from-address" name="from_address" type="email" required="" maxlength="254" placeholder="orders@example.com" value=(&current.from_address)
                            class="w-full rounded border border-neutral-300 px-3 py-2">
                        <label class="block text-sm" for="mail-from-name">"Send from name"</label>
                        <input id="mail-from-name" name="from_name" maxlength="80" placeholder=(&tenant.name) value=(current.from_name.as_deref().unwrap_or_default())
                            class="w-full rounded border border-neutral-300 px-3 py-2">
                        <label class="block text-sm" for="mail-test-to">"Send the test mail to"</label>
                        <input id="mail-test-to" name="test_to" type="email" required="" maxlength="254" value=(&test_to)
                            class="w-full rounded border border-neutral-300 px-3 py-2">
                        <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Test and save"</button>
                    </form>
                </section>
            }
        )
    })
}
