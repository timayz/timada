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
    router::{
        content::Form,
        error::{not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, component, view},
};

use super::super::super::own_address;
use super::{Kind, require_kind};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::tenants::{error_code, tenancy};
use crate::auth::mail::{send_rendered, shop_vars};
use crate::auth::{require_user, user_labels};
use crate::ui::{Shell, document, format_time, notice};

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
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(tenant, nav, ""),
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">(kind.label()) <span class="text-neutral-500">" · " (name)</span></h1>
            </header>
            <p class="mb-4 text-sm">
                <a class="underline" href=(href!(super::super::super::index, Slug(slug.clone())))>"Back to Mail"</a>
            </p>
            notice(error: error, message: message)

            <p class="mb-2 text-sm text-neutral-600">
                (kind.description()) " "
                if edited { "In " (name) " it goes out in your wording." } else { "In " (name) " it goes out in the built-in wording." }
                if is_default { " Shoppers whose language the shop does not mail in get this one." }
            </p>
            if siblings.len() > 1 {
                <nav aria-label="Languages" class="mb-4 flex flex-wrap gap-2 text-sm" data-mail-languages-nav="">
                    for (code, sibling, current) in &siblings {
                        if *current {
                            <span aria-current="page" class="rounded bg-neutral-900 px-2 py-1 text-white">(sibling)</span>
                        } else {
                            <a class="rounded border border-neutral-300 px-2 py-1 hover:bg-neutral-50"
                                href=(href!(index, Slug(slug.clone()), Kind(kind.key().to_owned()), Language(code.clone())))>(sibling)</a>
                        }
                    }
                </nav>
            }

            <div class="grid gap-6 lg:grid-cols-2">
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <form method="post" data-mail-language=(language)
                        action=(href!(submit, Slug(slug.clone()), Kind(kind.key().to_owned()), Language(language.to_owned()))) class="space-y-3">
                        <label class="block text-sm" for="mail-subject">"Subject"</label>
                        <input id="mail-subject" name="subject" required="" maxlength=(&max_subject) value=(subject)
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono text-sm">
                        <label class="block text-sm" for="mail-html">"Body (HTML)"</label>
                        <textarea id="mail-html" name="html" required="" rows="24" spellcheck="false"
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono text-xs">(html)</textarea>
                        <label class="block text-sm" for="mail-text">"Plain-text version"</label>
                        <p id="mail-text-hint" class="text-xs text-neutral-500">
                            "For mail apps that show no HTML. Leave it empty to derive it from the HTML."
                        </p>
                        <textarea id="mail-text" name="text" rows="10" spellcheck="false" aria-describedby="mail-text-hint"
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono text-xs">(text)</textarea>
                        if let Err(err) = checked {
                            <p role="alert" data-template-error="" class="rounded border border-red-300 bg-red-50 p-3 text-sm text-red-800">
                                (err.to_string())
                            </p>
                        }
                        <div class="flex flex-wrap gap-2">
                            <button name="action" value="save" class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Save"</button>
                            <button name="action" value="preview" class="rounded border border-neutral-300 px-4 py-2 hover:bg-neutral-50">"Preview"</button>
                            <button name="action" value="test" class="rounded border border-neutral-300 px-4 py-2 hover:bg-neutral-50">"Send me a test"</button>
                        </div>
                    </form>
                    if edited {
                        <form method="post" class="mt-4" action=(href!(reset::reset, Slug(slug.clone()), Kind(kind.key().to_owned()), Language(language.to_owned())))>
                            <button class="rounded border border-red-300 px-3 py-1 text-sm text-red-800 hover:bg-red-50">"Back to the built-in wording"</button>
                        </form>
                    }
                </section>

                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Preview"</h2>
                    if let Ok(rendered) = checked {
                        <p class="mb-2 text-sm"><span class="text-neutral-500">"Subject: "</span><span data-preview-subject="">(&rendered.subject)</span></p>
                        <iframe sandbox="" title="Preview of the mail" srcdoc=(&rendered.html)
                            class="h-[36rem] w-full rounded border border-neutral-200 bg-white"></iframe>
                        <h3 class="mt-4 mb-1 text-sm font-medium">"Plain-text version"</h3>
                        if let Some(text) = &rendered.text {
                            <pre data-preview-text="" class="max-h-64 overflow-auto whitespace-pre-wrap rounded border border-neutral-200 bg-neutral-50 p-3 font-mono text-xs">(text)</pre>
                        } else {
                            <p data-preview-text="" class="text-sm text-neutral-500">"Derived from the HTML."</p>
                        }
                        <p class="mt-2 text-xs text-neutral-500">"With made-up data; your mails use the shopper's."</p>
                    } else {
                        <p class="text-sm text-neutral-500">"Fix the template to see it."</p>
                    }
                </section>
            </div>

            <section class="mt-6 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Variables"</h2>
                <p class="mb-3 text-sm text-neutral-600">
                    "Write " <code>"{{ name }}"</code> " to print one; in the HTML what it holds is escaped, so it shows as text. "
                    "Lists go in " <code>"{% for … %} … {% endfor %}"</code> ", parts that may be missing in "
                    <code>"{% if … %} … {% endif %}"</code> ". Scripts, frames, forms and event handlers are refused, "
                    "and links start with https://."
                </p>
                <dl class="grid grid-cols-[14rem_1fr] gap-y-2 text-sm" data-template-variables="">
                    for variable in &variables {
                        <dt><code>"{{ " (variable.name) " }}"</code></dt>
                        <dd class="text-neutral-700">(variable.description)</dd>
                    }
                </dl>
            </section>

            if !changes.is_empty() {
                <section class="mt-6 rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"History"</h2>
                    <ul class="text-sm" data-template-history="">
                        for change in changes {
                            <li>
                                if change.reset { "Back to the built-in wording" } else { "Saved" }
                                " by " (&change.by) " on "
                                <time datetime=(change.at)>(format_time(change.at))</time>
                            </li>
                        }
                    </ul>
                </section>
            }
        )
    })
}
