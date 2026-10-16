//! `/t/{slug}/mail/templates/{kind}`: one of the shop's mails, worded the
//! merchant's way. The page shows the subject and HTML, the variables the
//! mail has, and a preview with made-up data. The form can preview the draft,
//! send it as a test to the member, or save it. Only checked templates are
//! saved (see [`timada_tenancy::validate`]).
//!
//! The preview is the merchant's own HTML. It sits in a sandboxed frame with
//! no scripts and an origin of its own, so nothing in it can reach the
//! dashboard.

pub mod reset;

use serde::Deserialize;
use timada_tenancy::{
    MAX_MAIL_SUBJECT_LEN, MailTemplateError, PERMISSION_MAIL_MANAGE, ShopMailKind, TenancyError,
    TenantState, render, validate,
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

use super::super::own_address;
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::app::tenants::{error_code, tenancy};
use crate::auth::mail::send_rendered;
use crate::auth::{require_user, user_labels};
use crate::ui::{document, format_time, notice};

module_param!(pub kind);

#[topcoat::router::query_params(error = bad_request)]
struct EditorQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn editor_url(
    cx: &Cx,
    slug: &str,
    kind: ShopMailKind,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, Slug(slug.to_owned()), Kind(kind.key().to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The mail named in the URL, or 404.
fn require_kind(cx: &Cx) -> Result<ShopMailKind> {
    ShopMailKind::from_key(path_param::<Kind>(cx)).ok_or_else(|| not_found().into())
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
    changes: Vec<Change>,
}

async fn load(cx: &Cx) -> Result<Page> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let kind = require_kind(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let changes: Vec<_> = access.tenant.mail_template_changes(kind).collect();
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
        changes,
    })
}

/// The draft rendered with made-up data, or why it can't be.
fn preview(
    kind: ShopMailKind,
    tenant: &TenantState,
    subject: &str,
    html: &str,
) -> std::result::Result<(String, String), MailTemplateError> {
    validate(kind, subject, html)?;
    let rendered = render(subject, html, &kind.sample(&tenant.name))?;
    Ok((rendered.subject, rendered.html))
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<EditorQuery>(cx)?;
    let page = load(cx).await?;
    let (subject, html) = match page.access.tenant.mail_template(page.kind) {
        Some(template) => (template.subject.clone(), template.html.clone()),
        None => (
            page.kind.default_subject().to_owned(),
            page.kind.default_html().to_owned(),
        ),
    };
    let checked = preview(page.kind, &page.access.tenant, &subject, &html);
    let Page {
        access,
        nav,
        kind,
        changes,
    } = page;
    Ok(view! {
        editor(
            tenant: &access.tenant,
            nav: &nav,
            kind: kind,
            subject: &subject,
            html: &html,
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
    let subject = form.subject.trim().to_owned();
    let checked = preview(page.kind, tenant, &subject, &html);
    let mut error = None;
    let mut message = None;
    match form.action.as_str() {
        "save" => {
            match tenancy(cx)
                .save_mail_template(
                    &page.access.user_id,
                    &tenant.id,
                    page.kind.key(),
                    &subject,
                    &html,
                )
                .await
            {
                Ok(()) => {
                    return Err(see_other(editor_url(
                        cx,
                        &tenant.slug,
                        page.kind,
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
            if let Ok((rendered_subject, rendered_html)) = &checked {
                let user = require_user(cx).await?;
                let to = own_address(user);
                let rendered = timada_tenancy::RenderedMail {
                    subject: format!("[Test] {rendered_subject}"),
                    html: rendered_html.clone(),
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
        changes,
    } = page;
    Ok(view! {
        editor(
            tenant: &access.tenant,
            nav: &nav,
            kind: kind,
            subject: &subject,
            html: &html,
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
    subject: &str,
    html: &str,
    checked: &std::result::Result<(String, String), MailTemplateError>,
    changes: &[Change],
    error: Option<&str>,
    message: Option<&str>,
) -> Result<impl View> {
    let slug = tenant.slug.clone();
    let title = format!("{} · Mail · {}", kind.label(), tenant.name);
    let edited = tenant.mail_template(kind).is_some();
    let variables = kind.variables();
    let max_subject = MAX_MAIL_SUBJECT_LEN.to_string();
    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">(kind.label())</h1>
                tenant_nav(tenant: tenant, nav: nav, current: "")
            </header>
            <p class="mb-4 text-sm">
                <a class="underline" href=(href!(super::super::index, Slug(slug.clone())))>"Back to Mail"</a>
            </p>
            notice(error: error, message: message)

            <p class="mb-4 text-sm text-neutral-600">
                (kind.description()) " "
                if edited { "It goes out in your wording." } else { "It goes out in the built-in wording." }
            </p>

            <div class="grid gap-6 lg:grid-cols-2">
                <section class="rounded border border-neutral-200 bg-white p-4">
                    <form method="post" action=(href!(submit, Slug(slug.clone()), Kind(kind.key().to_owned()))) class="space-y-3">
                        <label class="block text-sm" for="mail-subject">"Subject"</label>
                        <input id="mail-subject" name="subject" required="" maxlength=(&max_subject) value=(subject)
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono text-sm">
                        <label class="block text-sm" for="mail-html">"Body (HTML)"</label>
                        <textarea id="mail-html" name="html" required="" rows="24" spellcheck="false"
                            class="w-full rounded border border-neutral-300 px-3 py-2 font-mono text-xs">(html)</textarea>
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
                        <form method="post" class="mt-4" action=(href!(reset::reset, Slug(slug.clone()), Kind(kind.key().to_owned())))>
                            <button class="rounded border border-red-300 px-3 py-1 text-sm text-red-800 hover:bg-red-50">"Back to the built-in wording"</button>
                        </form>
                    }
                </section>

                <section class="rounded border border-neutral-200 bg-white p-4">
                    <h2 class="mb-2 text-lg font-medium">"Preview"</h2>
                    if let Ok((preview_subject, preview_html)) = checked {
                        <p class="mb-2 text-sm"><span class="text-neutral-500">"Subject: "</span><span data-preview-subject="">(preview_subject)</span></p>
                        <iframe sandbox="" title="Preview of the mail" srcdoc=(preview_html)
                            class="h-[36rem] w-full rounded border border-neutral-200 bg-white"></iframe>
                        <p class="mt-2 text-xs text-neutral-500">"With made-up data; your mails use the shopper's."</p>
                    } else {
                        <p class="text-sm text-neutral-500">"Fix the template to see it."</p>
                    }
                </section>
            </div>

            <section class="mt-6 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Variables"</h2>
                <p class="mb-3 text-sm text-neutral-600">
                    "Write " <code>"{{ name }}"</code> " to print one; what it holds is escaped, so it shows as text. "
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
