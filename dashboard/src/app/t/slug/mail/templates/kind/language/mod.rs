//! `/t/{slug}/mail/templates/{kind}/{language}`: one of the shop's mails,
//! in one of the languages the shop mails in, worded the merchant's way.
//! The page shows the subject, the HTML and an optional plain-text part
//! (derived from the HTML when left empty), the variables the mail has, and
//! a preview with made-up data. The form can preview the draft, send it as
//! a test to the member, or save it. Only checked templates are saved (see
//! [`timada_tenancy::validate`] and [`timada_tenancy::validate_text`]).
//!
//! The preview is the merchant's own HTML. It sits in a sandboxed frame with
//! no scripts and an origin of its own, so nothing in it can reach the
//! dashboard.

pub mod reset;

use serde::Deserialize;
use timada_tenancy::{
    MAX_MAIL_SUBJECT_LEN, MailTemplateError, PERMISSION_MAIL_MANAGE, RenderedMail,
    SaveMailTemplate, ShopMailKind, TenancyError, TenantState, language_name, render, validate,
    validate_text,
};
use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    router::{
        content::Form,
        error::{not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, class, component, view},
};

use super::super::super::own_address;
use super::{Kind, require_kind};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::tenants::{error_code, tenancy};
use crate::auth::mail::{send_rendered, shop_vars};
use crate::auth::{require_user, user_labels};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::badge::{BadgeVariant, badge_variants};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::textarea::textarea;
use crate::ui::{Shell, document, format_time, icons, notice, page_header, section_card};

module_param!(pub language);

#[topcoat::router::query_params(error = bad_request)]
struct EditorQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn editor_url(
    cx: &Cx,
    slug: &str,
    kind: ShopMailKind,
    language: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        Slug(slug.to_owned()),
        Kind(kind.key().to_owned()),
        Language(language.to_owned())
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// The language named in the URL, lowercased, or 404 when the shop does
/// not mail in it.
pub fn require_language(cx: &Cx, tenant: &TenantState) -> Result<String> {
    let language = path_param::<Language>(cx).trim().to_ascii_lowercase();
    if tenant.mails_in(&language) {
        Ok(language)
    } else {
        Err(not_found().into())
    }
}

/// One change to the mail's wording, as the history lists it.
pub struct Change {
    by: String,
    at: u64,
    reset: bool,
}

/// What the page shows besides the draft.
struct Page {
    access: TenantAccess,
    nav: TenantNav,
    kind: ShopMailKind,
    language: String,
    changes: Vec<Change>,
}

async fn load(cx: &Cx) -> Result<Page> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let kind = require_kind(cx)?;
    let language = require_language(cx, &access.tenant)?;
    let nav = TenantNav::of(cx, &access).await?;
    let changes: Vec<_> = access
        .tenant
        .mail_template_changes(kind, &language)
        .collect();
    let ids: Vec<String> = changes.iter().map(|change| change.by.clone()).collect();
    let labels = user_labels(cx, &ids).await?;
    let changes = changes
        .into_iter()
        .map(|change| Change {
            by: labels
                .get(&change.by)
                .cloned()
                .unwrap_or_else(|| change.by.clone()),
            at: change.at,
            reset: change.reset,
        })
        .collect();
    Ok(Page {
        access,
        nav,
        kind,
        language,
        changes,
    })
}

/// The draft rendered with made-up data, or why it can't be. An empty
/// `text` is the text derived from the HTML.
fn preview(
    cx: &Cx,
    kind: ShopMailKind,
    tenant: &TenantState,
    subject: &str,
    html: &str,
    text: &str,
) -> std::result::Result<RenderedMail, MailTemplateError> {
    validate(kind, subject, html)?;
    let text = (!text.trim().is_empty()).then_some(text);
    if let Some(text) = text {
        validate_text(kind, text)?;
    }
    render(subject, html, text, &kind.sample(shop_vars(cx, tenant)))
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<EditorQuery>(cx)?;
    let page = load(cx).await?;
    let (subject, html, text) = match page
        .access
        .tenant
        .mail_template_in(page.kind, &page.language)
    {
        Some(template) => (
            template.subject.clone(),
            template.html.clone(),
            template.text.clone().unwrap_or_default(),
        ),
        None => (
            page.kind.default_subject().to_owned(),
            page.kind.default_html().to_owned(),
            String::new(),
        ),
    };
    let checked = preview(cx, page.kind, &page.access.tenant, &subject, &html, &text);
    let Page {
        access,
        nav,
        kind,
        language,
        changes,
    } = page;
    Ok(view! {
        editor(
            tenant: &access.tenant,
            nav: &nav,
            kind: kind,
            language: &language,
            subject: &subject,
            html: &html,
            text: &text,
            checked: &checked,
            changes: &changes,
            error: query.error.as_deref(),
            message: query.message.as_deref(),
        )
    })
}

#[derive(Deserialize)]
pub struct EditorForm {
    subject: String,
    html: String,
    /// The plain-text part; empty derives it from the HTML.
    #[serde(default)]
    text: String,
    /// `preview`, `test` or `save`.
    action: String,
}

/// Previews, tests or saves the draft. A saved draft goes back to the page;
/// otherwise the page shows the draft as it was sent, so nothing typed is
/// lost.
#[page(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<EditorForm>) -> Result<impl View> {
    let page = load(cx).await?;
    let tenant = &page.access.tenant;
    // Browsers send textarea line breaks as CRLF.
    let html = form.html.replace("\r\n", "\n");
    let text = form.text.replace("\r\n", "\n");
    let subject = form.subject.trim().to_owned();
    let checked = preview(cx, page.kind, tenant, &subject, &html, &text);
    let mut error = None;
    let mut message = None;
    match form.action.as_str() {
        "save" => {
            match tenancy(cx)
                .save_mail_template(
                    &page.access.user_id,
                    &tenant.id,
                    page.kind.key(),
                    &page.language,
                    SaveMailTemplate {
                        subject: subject.clone(),
                        html: html.clone(),
                        text: Some(text.clone()),
                    },
                )
                .await
            {
                Ok(()) => {
                    return Err(see_other(editor_url(
                        cx,
                        &tenant.slug,
                        page.kind,
                        &page.language,
                        None,
                        Some("Saved: the mail goes out in this wording from now on."),
                    ))
                    .into());
                }
                // The page shows the template's own error under the form.
                Err(TenancyError::InvalidMailTemplate(_)) => {
                    error = Some("invalid_mail_template");
                }
                Err(err) if err.is_user_facing() => error = Some(error_code(&err)),
                Err(err) => return Err(err.into()),
            }
        }
        "test" => {
            if let Ok(rendered) = &checked {
                let user = require_user(cx).await?;
                let to = own_address(user);
                let rendered = RenderedMail {
                    subject: format!("[Test] {}", rendered.subject),
                    ..rendered.clone()
                };
                match send_rendered(cx, tenant, &to, rendered).await {
                    Ok(()) => message = Some(format!("Test mail sent to {to}.")),
                    Err(err) => {
                        tracing::warn!(tenant = %tenant.id, error = %err, "test mail failed");
                        error = Some("mail_test_failed");
                    }
                }
            }
        }
        _ => {}
    }
    let Page {
        access,
        nav,
        kind,
        language,
        changes,
    } = page;
    Ok(view! {
        editor(
            tenant: &access.tenant,
            nav: &nav,
            kind: kind,
            language: &language,
            subject: &subject,
            html: &html,
            text: &text,
            checked: &checked,
            changes: &changes,
            error: error,
            message: message.as_deref(),
        )
    })
}

#[component]
async fn editor(
    tenant: &TenantState,
    nav: &TenantNav,
    kind: ShopMailKind,
    language: &str,
    subject: &str,
    html: &str,
    text: &str,
    checked: &std::result::Result<RenderedMail, MailTemplateError>,
    changes: &[Change],
    error: Option<&str>,
    message: Option<&str>,
) -> Result<impl View> {
    let slug = tenant.slug.clone();
    let name = language_name(language).unwrap_or(language);
    let title = format!("{} · {} · Mail · {}", kind.label(), name, tenant.name);
    let edited = tenant.mail_template_in(kind, language).is_some();
    let is_default = tenant.mail_default_language() == language;
    // The same mail in the shop's other languages.
    let siblings: Vec<(String, &str, bool)> = tenant
        .mail_languages()
        .into_iter()
        .map(|code| {
            (
                code.to_owned(),
                language_name(code).unwrap_or(code),
                code == language,
            )
        })
        .collect();
    let variables = kind.variables();
    let max_subject = MAX_MAIL_SUBJECT_LEN.to_string();
    let heading = format!("{} · {}", kind.label(), name);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(tenant, nav, ""),
            page_header(
                title: &heading,
                subtitle: Some(kind.description()),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(super::super::super::index, Slug(slug.clone())))>"Back to Mail"</a>
            )
            notice(error: error, message: message)

            <p class="mb-4 text-sm text-muted-foreground">
                if edited { "In " (name) " it goes out in your wording." } else { "In " (name) " it goes out in the built-in wording." }
                if is_default { " Shoppers whose language the shop does not mail in get this one." }
            </p>
            if siblings.len() > 1 {
                <nav aria-label="Languages" class="mb-6 flex flex-wrap gap-2" data-mail-languages-nav="">
                    for (code, sibling, current) in &siblings {
                        if *current {
                            <span aria-current="page" class=(badge_variants(BadgeVariant::Tinted))>(sibling)</span>
                        } else {
                            <a class=(class!(badge_variants(BadgeVariant::Outline), "hover:bg-foreground/5"))
                                href=(href!(index, Slug(slug.clone()), Kind(kind.key().to_owned()), Language(code.clone())))>(sibling)</a>
                        }
                    }
                </nav>
            }

            <div class="grid gap-6 lg:grid-cols-2">
                section_card(
                    attrs: attributes! { class="mb-0" },
                    <form method="post" data-mail-language=(language)
                        action=(href!(submit, Slug(slug.clone()), Kind(kind.key().to_owned()), Language(language.to_owned()))) class="flex flex-col gap-4">
                        field(
                            field_label(attrs: attributes! { for="mail-subject" }, "Subject")
                            input(attrs: attributes! {
                                id="mail-subject" name="subject" required="" maxlength=(&max_subject) value=(subject) class="font-mono"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-html" }, "Body (HTML)")
                            textarea(
                                attrs: attributes! { id="mail-html" name="html" required="" rows="24" spellcheck="false" class="font-mono text-xs" },
                                (html)
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="mail-text" }, "Plain-text version")
                            field_description(
                                attrs: attributes! { id="mail-text-hint" },
                                "For mail apps that show no HTML. Leave it empty to derive it from the HTML."
                            )
                            textarea(
                                attrs: attributes! {
                                    id="mail-text" name="text" rows="10" spellcheck="false" aria-describedby="mail-text-hint" class="font-mono text-xs"
                                },
                                (text)
                            )
                        )
                        if let Err(err) = checked {
                            alert(
                                variant: AlertVariant::Destructive,
                                attrs: attributes! { role="alert" data-template-error="" },
                                icon(data: icons::ERROR)
                                alert_description((err.to_string()))
                            )
                        }
                        <div class="flex flex-wrap gap-2">
                            button(attrs: attributes! { type="submit" name="action" value="save" }, "Save")
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" name="action" value="preview" }, "Preview")
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" name="action" value="test" }, "Send me a test")
                        </div>
                    </form>
                    if edited {
                        <form method="post" class="mt-4" action=(href!(reset::reset, Slug(slug.clone()), Kind(kind.key().to_owned()), Language(language.to_owned())))>
                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Back to the built-in wording")
                        </form>
                    }
                )

                section_card(
                    title: Some("Preview"),
                    attrs: attributes! { class="mb-0" },
                    if let Ok(rendered) = checked {
                        <p class="mb-2 text-sm"><span class="text-muted-foreground">"Subject: "</span><span data-preview-subject="">(&rendered.subject)</span></p>
                        // The mail's own HTML rarely paints a background: white under it
                        // whatever the dashboard's scheme, as a mail app would.
                        <iframe sandbox="" title="Preview of the mail" srcdoc=(&rendered.html)
                            class="h-[36rem] w-full rounded-lg border border-border bg-white"></iframe>
                        <h3 class="mt-4 mb-1 text-sm font-medium">"Plain-text version"</h3>
                        if let Some(text) = &rendered.text {
                            <pre data-preview-text="" class="max-h-64 overflow-auto whitespace-pre-wrap rounded-lg border border-border bg-muted p-3 font-mono text-xs">(text)</pre>
                        } else {
                            <p data-preview-text="" class="text-sm text-muted-foreground">"Derived from the HTML."</p>
                        }
                        <p class="mt-2 text-xs text-muted-foreground">"With made-up data; your mails use the shopper's."</p>
                    } else {
                        <p class="text-sm text-muted-foreground">"Fix the template to see it."</p>
                    }
                )
            </div>

            section_card(
                title: Some("Variables"),
                attrs: attributes! { class="mt-6" },
                <p class="mb-3 text-sm text-muted-foreground">
                    "Write " <code>"{{ name }}"</code> " to print one; in the HTML what it holds is escaped, so it shows as text. "
                    "Lists go in " <code>"{% for … %} … {% endfor %}"</code> ", parts that may be missing in "
                    <code>"{% if … %} … {% endif %}"</code> ". Scripts, frames, forms and event handlers are refused, "
                    "and links start with https://."
                </p>
                <dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[14rem_minmax(0,1fr)]" data-template-variables="">
                    for variable in &variables {
                        <dt><code>"{{ " (variable.name) " }}"</code></dt>
                        <dd class="text-muted-foreground">(variable.description)</dd>
                    }
                </dl>
            )

            if !changes.is_empty() {
                section_card(
                    title: Some("History"),
                    <ul class="space-y-1 text-sm" data-template-history="">
                        for change in changes {
                            <li>
                                if change.reset { "Back to the built-in wording" } else { "Saved" }
                                " by " (&change.by) " on "
                                <time datetime=(change.at)>(format_time(change.at))</time>
                            </li>
                        }
                    </ul>
                )
            }
        )
    })
}
