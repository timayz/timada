//! `/t/{slug}/mail`: the mail server the shop's mails to its shoppers go
//! out through, for members with `mail:manage`. Without one they go out
//! through the platform's server, under the shop's name. Saving sends a test
//! mail through the new settings first: what cannot deliver is not kept.

pub mod branding;
pub mod configure;
pub mod languages;
pub mod remove;
pub mod templates;

use std::sync::Arc;

use timada_identity::UserState;
use timada_tenancy::{
    LANGUAGES, MAIL_SECURITY_NONE, MAIL_SECURITY_STARTTLS, MAIL_SECURITY_TLS, MAX_MAIL_FOOTER_LEN,
    PERMISSION_MAIL_MANAGE, ShopMailKind, language_name,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::auth::{require_user, user_labels};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::textarea::textarea;
use crate::media::Media;
use crate::shop_mail::ShopMail;
use crate::ui::{
    LINK, Shell, document, fact, facts, format_time, icons, notice, page_header, section_card,
};

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
    let shop_languages: Vec<String> = tenant
        .mail_languages()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let default_language = tenant.mail_default_language().to_owned();
    let editors: Vec<String> = ShopMailKind::ALL
        .iter()
        .flat_map(|kind| {
            shop_languages
                .iter()
                .filter_map(|language| tenant.mail_template_in(*kind, language))
        })
        .map(|template| template.saved_by.clone())
        .collect();
    let labels = user_labels(cx, &editors).await?;
    let branding = tenant.mail_branding.clone().unwrap_or_default();
    let logo_url = branding
        .logo_file
        .as_deref()
        .map(|file| app_context::<Arc<Media>>(cx).url(&tenant.id, file));
    let max_footer = MAX_MAIL_FOOTER_LEN.to_string();
    // Each mail, and for each language the shop mails in, who worded it.
    type Wording = (String, &'static str, Option<(String, u64)>);
    let mails: Vec<(ShopMailKind, Vec<Wording>)> = ShopMailKind::ALL
        .into_iter()
        .map(|kind| {
            let wordings = shop_languages
                .iter()
                .map(|language| {
                    let edited = tenant.mail_template_in(kind, language).map(|template| {
                        let by = labels
                            .get(&template.saved_by)
                            .cloned()
                            .unwrap_or_else(|| template.saved_by.clone());
                        (by, template.saved_at)
                    });
                    (
                        language.clone(),
                        language_name(language).unwrap_or("Unknown"),
                        edited,
                    )
                })
                .collect();
            (kind, wordings)
        })
        .collect();

    let server_title = if settings.is_some() {
        "Change the server"
    } else {
        "Send from your own server"
    };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "mail"),
            page_header(title: "Mail", subtitle: Some("The mails your shop sends its shoppers, and the server they go out through."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if !enabled {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description(
                        "Shop mail servers are not enabled on this platform: the operator has to set them up first."
                    )
                )
            }

            section_card(
                title: Some("Mail server"),
                if let Some(settings) = &settings {
                    <div class="mb-4" data-mail-server="own">
                        facts(
                            fact(term: "Server", <code data-mail-host="">(&settings.host) ":" (settings.port)</code>)
                            fact(term: "Security", (security_label(&settings.security)))
                            fact(term: "User name", (settings.username.as_deref().unwrap_or("None")))
                            fact(term: "Password", (if settings.password_sealed.is_some() { "Saved, never shown again" } else { "None" }))
                            fact(
                                term: "Sent from",
                                attrs: attributes! { data-mail-from="" },
                                (settings.from_name.as_deref().unwrap_or(&tenant.name))
                                " <" (&settings.from_address) ">"
                            )
                            fact(term: "Since", <time datetime=(settings.configured_at)>(format_time(settings.configured_at))</time>)
                        )
                    </div>
                    <form method="post" action=(href!(remove::remove, Slug(slug.clone())))>
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Use the platform's server")
                    </form>
                } else {
                    <p class="text-sm text-muted-foreground" data-mail-server="platform">
                        "Not set up: your order confirmations, review requests and sign-in codes go out through the platform's server, under the name "
                        <strong class="text-foreground">(&tenant.name)</strong> "."
                    </p>
                }
            )

            section_card(
                title: Some("Languages"),
                description: Some("Your shop mails each shopper in the language they browse in, when you have worded the mail in it; otherwise in the default language. The built-in wording is English."),
                attrs: attributes! { data-mail-languages="" },
                <form method="post" action=(href!(languages::save, Slug(slug.clone()))) class="flex flex-col gap-4">
                    field(
                        attrs: attributes! { class="max-w-xs" },
                        field_label(attrs: attributes! { for="mail-default-language" }, "Default language")
                        select(
                            attrs: attributes! { id="mail-default-language" name="default" },
                            for (code, name) in LANGUAGES {
                                <option value=(code) selected=(*code == default_language)>(name)</option>
                            }
                        )
                    )
                    <fieldset class="min-w-0">
                        <legend class="text-sm font-medium">"Also in"</legend>
                        <div class="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 sm:grid-cols-3 md:grid-cols-4">
                            for (code, name) in LANGUAGES {
                                <label class="flex items-center gap-2 py-0.5 text-sm" data-language-option=(code)>
                                    checkbox(attrs: attributes! {
                                        name=(format!("{}{code}", languages::LANGUAGE_FIELD_PREFIX)) value="on"
                                        checked=(*code != default_language && shop_languages.iter().any(|language| language == code))
                                    })
                                    (name)
                                </label>
                            }
                        </div>
                    </fieldset>
                    <div>
                        button(attrs: attributes! { type="submit" }, "Save languages")
                    </div>
                </form>
            )

            section_card(
                title: Some("Mails to shoppers"),
                description: Some("Word the mails your shop sends in your own way, in each of its languages. A mail you have not changed goes out as below."),
                attrs: attributes! { data-mail-templates="" },
                <ul class="divide-y divide-border">
                    for (kind, wordings) in &mails {
                        <li class="py-3" data-mail-template=(kind.key())>
                            <a class=(LINK)
                                href=(href!(templates::kind::index, Slug(slug.clone()), templates::kind::Kind(kind.key().to_owned())))>
                                (kind.label())
                            </a>
                            <p class="text-sm text-muted-foreground">(kind.description())</p>
                            <ul class="mt-2 space-y-1">
                                for (code, name, edited) in wordings {
                                    <li class="flex items-center justify-between gap-4 text-sm" data-language=(code)>
                                        <a class=(LINK)
                                            href=(href!(templates::kind::language::index, Slug(slug.clone()), templates::kind::Kind(kind.key().to_owned()), templates::kind::language::Language(code.clone())))>
                                            (name)
                                        </a>
                                        <span class="text-muted-foreground">
                                            if let Some((by, at)) = edited {
                                                "Edited by " (by) " on " <time datetime=(at)>(format_time(*at))</time>
                                            } else {
                                                "Built-in wording"
                                            }
                                        </span>
                                    </li>
                                }
                            </ul>
                        </li>
                    }
                </ul>
            )

            section_card(
                title: Some("Look of your mails"),
                attrs: attributes! { data-mail-branding="" },
                <p class="mb-4 text-sm text-muted-foreground">
                    "A logo, an accent colour and a footer, shared by all the mails above. In your own wording, use "
                    <code>"{{ shop.logo_url }}"</code> ", " <code>"{{ shop.accent }}"</code> " and " <code>"{{ shop.footer }}"</code> "."
                </p>
                <form method="post" enctype="multipart/form-data" action=(href!(branding::save, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                    if let Some(logo_url) = &logo_url {
                        <img src=(logo_url) alt="Your logo" data-mail-logo="" class="max-h-16 self-start rounded-lg border border-border bg-card p-1">
                        <label class="flex items-center gap-2 text-sm">
                            checkbox(attrs: attributes! { name="remove_logo" value="1" })
                            "Remove the logo"
                        </label>
                    }
                    field(
                        field_label(
                            attrs: attributes! { for="mail-logo" },
                            if logo_url.is_some() { "Replace the logo" } else { "Logo" }
                        )
                        input(attrs: attributes! { id="mail-logo" name="logo" type="file" accept="image/png,image/jpeg,image/webp,image/gif" })
                    )
                    field(
                        field_label(attrs: attributes! { for="mail-accent" }, "Accent colour")
                        input(attrs: attributes! {
                            id="mail-accent" name="accent" maxlength="7" placeholder="#1a73e8" pattern="#[0-9a-fA-F]{6}"
                            value=(branding.accent.as_deref().unwrap_or_default()) class="w-40 font-mono"
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="mail-footer" }, "Footer")
                        textarea(
                            attrs: attributes! {
                                id="mail-footer" name="footer" rows="3" maxlength=(&max_footer)
                                placeholder="Your shop's address, a line about returns…"
                            },
                            (branding.footer.as_deref().unwrap_or_default())
                        )
                    )
                    <div>
                        button(attrs: attributes! { type="submit" }, "Save the look")
                    </div>
                </form>
                if tenant.mail_branding.is_some() {
                    <form method="post" class="mt-4" action=(href!(branding::clear::clear, Slug(slug.clone())))>
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Back to the plain look")
                    </form>
                }
            )

            if enabled {
                section_card(
                    title: Some(server_title),
                    description: Some("Your mail provider gives these settings for sending (SMTP). Saving sends a test mail through them first: they are kept only if it goes out."),
                    <form method="post" action=(href!(configure::configure, Slug(slug.clone()))) class="grid max-w-3xl gap-4 md:grid-cols-2">
                        field(
                            field_label(attrs: attributes! { for="mail-host" }, "Server")
                            input(attrs: attributes! {
                                id="mail-host" name="host" required="" maxlength="253" placeholder="smtp.example.com" value=(&current.host) class="font-mono"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-port" }, "Port")
                            input(attrs: attributes! {
                                id="mail-port" name="port" type="number" min="1" max="65535" required="" value=(&port) class="font-mono"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-security" }, "Security")
                            select(
                                attrs: attributes! { id="mail-security" name="security" },
                                for (value, label) in [
                                    (MAIL_SECURITY_STARTTLS, "STARTTLS (usually port 587)"),
                                    (MAIL_SECURITY_TLS, "TLS (usually port 465)"),
                                    (MAIL_SECURITY_NONE, "None (not encrypted)"),
                                ] {
                                    <option value=(value) selected=(security == value)>(label)</option>
                                }
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-username" }, "User name")
                            input(attrs: attributes! {
                                id="mail-username" name="username" maxlength="254" autocomplete="off"
                                value=(current.username.as_deref().unwrap_or_default()) class="font-mono"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-password" }, "Password")
                            input(attrs: attributes! {
                                id="mail-password" name="password" type="password" autocomplete="off"
                                placeholder=(if has_password { "Leave blank to keep the saved one" } else { "" }) class="font-mono"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-from-address" }, "Send from address")
                            input(attrs: attributes! {
                                id="mail-from-address" name="from_address" type="email" required="" maxlength="254"
                                placeholder="orders@example.com" value=(&current.from_address)
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-from-name" }, "Send from name")
                            input(attrs: attributes! {
                                id="mail-from-name" name="from_name" maxlength="80" placeholder=(&tenant.name)
                                value=(current.from_name.as_deref().unwrap_or_default())
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-test-to" }, "Send the test mail to")
                            input(attrs: attributes! {
                                id="mail-test-to" name="test_to" type="email" required="" maxlength="254" value=(&test_to)
                            })
                        )
                        <div class="md:col-span-2">
                            button(attrs: attributes! { type="submit" }, "Test and save")
                        </div>
                    </form>
                )
            }
        )
    })
}
