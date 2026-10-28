//! `/t/{slug}/mail`, for members with `mail:manage`: the shop's own mail
//! servers and which one sends what, then the wording and look of its
//! mails. The mails to its shoppers go out through the server picked for
//! them, else through the platform's server under the shop's name; its
//! marketing only ever goes out through the server picked for it.

pub mod branding;
pub mod languages;
pub mod servers;
pub mod templates;

use std::sync::Arc;

use timada_identity::UserState;
use timada_tenancy::{
    LANGUAGES, MAIL_SECURITY_STARTTLS, MAX_MAIL_FOOTER_LEN, MAX_MAIL_SERVERS, MailServerSettings,
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
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::components::textarea::textarea;
use crate::media::Media;
use crate::shop_mail::ShopMail;
use crate::ui::{
    LINK, Shell, Tone, document, fact, facts, format_time, icons, notice, page_header,
    section_card, status_badge,
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
    let title = format!("Mail · {}", tenant.name);
    // What goes out through which server here: with shop servers off, the
    // shop's mails through the platform's and no marketing.
    let shop_mails = tenant.shop_mails_server().filter(|_| enabled).cloned();
    let marketing = tenant.marketing_server().filter(|_| enabled).cloned();
    // What the merchant picked, for the form.
    let picked_shop_mails = tenant.shop_mails_server().map(|server| server.id.clone());
    let picked_marketing = tenant.marketing_server().map(|server| server.id.clone());
    let servers: Vec<(MailServerSettings, bool, bool)> = tenant
        .mail_servers
        .iter()
        .map(|server| {
            let (shop_mails, marketing) = servers::picked_for(&tenant, &server.id);
            (server.clone(), shop_mails, marketing)
        })
        .collect();
    let add_title = if servers.is_empty() {
        "Send from your own server"
    } else {
        "Add a server"
    };
    let full = servers.len() >= MAX_MAIL_SERVERS;
    let add_action = href!(servers::add::add, Slug(slug.clone())).resolve(cx);
    let blank = MailServerSettings {
        port: 587,
        security: MAIL_SECURITY_STARTTLS.to_owned(),
        ..Default::default()
    };
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
                title: Some("Mail servers"),
                description: Some("The servers your shop sends through, and which one sends what. Without one picked, the mails to your shoppers go out through the platform's server; marketing never does."),
                attrs: attributes! { data-mail-servers="" },
                facts(
                    fact(
                        term: "Shop mails go out through",
                        attrs: attributes! { data-mail-server=(if shop_mails.is_some() { "own" } else { "platform" }) },
                        if let Some(server) = &shop_mails {
                            <span data-mail-from="">(server.from_name.as_deref().unwrap_or(&tenant.name)) " <" (&server.from_address) ">"</span>
                            " · " <code data-mail-host="">(&server.host) ":" (server.port)</code>
                        } else {
                            "The platform's server, under the name " <strong>(&tenant.name)</strong>
                        }
                    )
                    fact(
                        term: "Marketing goes out through",
                        attrs: attributes! { data-mail-marketing=(if marketing.is_some() { "own" } else { "none" }) },
                        if let Some(server) = &marketing {
                            (server.from_name.as_deref().unwrap_or(&tenant.name)) " <" (&server.from_address) ">"
                            " · " <code>(&server.host) ":" (server.port)</code>
                        } else {
                            "No server: newsletter campaigns and automated mails wait until you pick one."
                        }
                    )
                )
                if !servers.is_empty() {
                    <div class="mt-4">
                        table(
                            table_header(
                                table_row(
                                    table_head("Name")
                                    table_head("Sends from")
                                    table_head("Host")
                                    table_head("Picked for")
                                )
                            )
                            table_body(
                                for (server, shop_mails, marketing) in &servers {
                                    table_row(
                                        attrs: attributes! { data-mail-server-row=(&server.id) },
                                        table_cell(
                                            <a class=(LINK) href=(href!(servers::server_id::index, Slug(slug.clone()), servers::server_id::ServerId(server.id.clone())))>(server.label())</a>
                                        )
                                        table_cell((&server.from_address))
                                        table_cell(<code>(&server.host) ":" (server.port)</code>)
                                        table_cell(
                                            <div class="flex flex-wrap gap-1">
                                                if *shop_mails {
                                                    status_badge(tone: Tone::Success, attrs: attributes! { data-sends="shop-mails" }, "Shop mails")
                                                }
                                                if *marketing {
                                                    status_badge(tone: Tone::Info, attrs: attributes! { data-sends="marketing" }, "Marketing")
                                                }
                                            </div>
                                        )
                                    )
                                }
                            )
                        )
                    </div>
                }
            )

            if !servers.is_empty() {
                section_card(
                    title: Some("Which server sends what"),
                    description: Some("The mails to your shoppers can go through any of your servers, or the platform's. Marketing only ever goes through one of yours."),
                    attrs: attributes! { data-mail-picks="" },
                    <form method="post" action=(href!(servers::pick::pick, Slug(slug.clone()))) class="grid max-w-3xl gap-4 md:grid-cols-2">
                        field(
                            field_label(attrs: attributes! { for="mail-shop-mails" }, "Shop mails go through")
                            select(
                                attrs: attributes! { id="mail-shop-mails" name="shop_mails" },
                                <option value=(servers::pick::PLATFORM) selected=(picked_shop_mails.is_none())>"The platform's server, under the shop's name"</option>
                                for (server, _, _) in &servers {
                                    <option value=(&server.id) selected=(picked_shop_mails.as_deref() == Some(server.id.as_str()))>
                                        (server.label()) " · " (&server.host)
                                    </option>
                                }
                            )
                            <p class="text-sm text-muted-foreground">"Order confirmations, review requests, sign-in codes and newsletter confirmations."</p>
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-marketing" }, "Marketing goes through")
                            select(
                                attrs: attributes! { id="mail-marketing" name="marketing" },
                                <option value=(servers::pick::NONE) selected=(picked_marketing.is_none())>"None: campaigns and automated mails wait"</option>
                                for (server, _, _) in &servers {
                                    <option value=(&server.id) selected=(picked_marketing.as_deref() == Some(server.id.as_str()))>
                                        (server.label()) " · " (&server.host)
                                    </option>
                                }
                            )
                            <p class="text-sm text-muted-foreground">"Newsletter campaigns and the automated mails of your scenarios."</p>
                        )
                        <div class="md:col-span-2">
                            button(attrs: attributes! { type="submit" }, "Save choices")
                        </div>
                    </form>
                )
            }

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

            if enabled && !full {
                section_card(
                    title: Some(add_title),
                    description: Some("Your mail provider gives these settings for sending (SMTP). Saving sends a test mail through them first: they are kept only if it goes out."),
                    servers::server_form(action: add_action, current: &blank, shop_name: &tenant.name, test_to: &test_to)
                )
            } else if enabled {
                section_card(
                    title: Some("Add a server"),
                    <p class="text-sm text-muted-foreground">"Your shop keeps at most ten mail servers: delete one to add another."</p>
                )
            }
        )
    })
}
