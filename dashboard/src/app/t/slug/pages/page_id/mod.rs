//! `/t/{slug}/pages/{page_id}`: one page, read from its own stream (so an
//! edit shows at once). Members with `pages:manage` write it here beside its
//! preview, and the forms below publish it, place it in the footer, set what
//! search engines show, move it to another address, add pictures and delete
//! it.
//!
//! The preview is the HTML the storefront shows, already cleaned by
//! [`render_markdown`]; it still sits in a sandboxed frame with no scripts,
//! as the newsletter's previews do.

pub mod delete;
pub mod footer;
pub mod publish;
pub mod remove_image;
pub mod rename;
pub mod seo;
pub mod unpublish;
pub mod upload;

use std::sync::Arc;

use serde::Deserialize;
use timada_pages::{
    MAX_BODY_LEN, MAX_FOOTER_POSITION, MAX_IMAGES_PER_PAGE, MAX_META_DESCRIPTION_LEN,
    MAX_META_TITLE_LEN, MAX_SLUG_LEN, MAX_TITLE_LEN, PageState, PagesError, render_markdown,
};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, class, component, view},
};

use super::{error_code, pages, pages_url, status_of};
use crate::app::auth::Flash;
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::card::CARD;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::label::label;
use crate::components::textarea::textarea;
use crate::media::Media;
use crate::ui::{Shell, document, format_time, notice, page_header, section_card, status_badge};

module_param!(pub page_id);

#[topcoat::router::query_params(error = bad_request)]
struct PageQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn page_url(
    cx: &Cx,
    slug: &str,
    page_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, Slug(slug.to_owned()), PageId(page_id.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The page the URL names, or 404.
pub async fn require_page(access: &TenantAccess, cx: &Cx) -> Result<PageState> {
    let id = path_param::<PageId>(cx);
    pages(access)?
        .load_page(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// Runs one change of the page the URL names for a member with
/// `pages:manage`, then goes back to it with `message`, or with the code of
/// the refusal.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_pages::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), PagesError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let page_id = path_param::<PageId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = pages(&access)?;
    match run(&gateway, &access.user_id, &page_id).await {
        Ok(()) => Ok(see_other(page_url(
            cx,
            &slug,
            &page_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(page_url(
            cx,
            &slug,
            &page_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<PageQuery>(cx)?;
    let page = require_page(&access, cx).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let nav = TenantNav::of(cx, &access).await?;
    let media_url = app_context::<Arc<Media>>(cx).tenant_url(&access.tenant.id);
    let preview = preview_document(&page.title, &render_markdown(&page.body, &media_url));
    let tenant = access.tenant;
    let slug = tenant.slug.clone();
    let title = format!("{} · Pages · {}", page.title, tenant.name);
    let back = pages_url(cx, &slug, None, None);
    let (status, tone) = status_of(page.published);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "pages"),
            page_header(
                title: &page.title,
                subtitle: Some("Written in Markdown; the storefront shows it once it is published."),
                status_badge(tone: tone, attrs: attributes! { data-page-status=(status) }, (status))
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Pages"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            page_screen(
                slug: &slug,
                page: &page,
                title: &page.title,
                body: &page.body,
                preview: &preview,
                media_url: &media_url,
                can_manage: can_manage,
            )
        )
    })
}

/// What the editor posts.
#[derive(Deserialize)]
pub struct EditForm {
    title: String,
    #[serde(default)]
    body: String,
    /// `preview` or `save`.
    action: String,
}

/// Saves the text, or shows it in the preview without saving it. A refused
/// save shows the text as it was sent, so nothing typed is lost.
#[page(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<EditForm>) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let page = require_page(&access, cx).await?;
    let body = form.body.replace("\r\n", "\n");
    let mut error = None;
    if form.action == "save" {
        match pages(&access)?
            .edit_page(&access.user_id, &page.id, &form.title, &body)
            .await
        {
            Ok(()) => {
                return Err(see_other(page_url(
                    cx,
                    &access.tenant.slug,
                    &page.id,
                    None,
                    Some("Page saved."),
                ))
                .into());
            }
            Err(err) if err.is_user_facing() => error = Some(error_code(&err)),
            Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
        }
    }
    let nav = TenantNav::of(cx, &access).await?;
    let media_url = app_context::<Arc<Media>>(cx).tenant_url(&access.tenant.id);
    let preview = preview_document(&form.title, &render_markdown(&body, &media_url));
    let tenant = access.tenant;
    let slug = tenant.slug.clone();
    let title = format!("{} · Pages · {}", page.title, tenant.name);
    let back = pages_url(cx, &slug, None, None);
    let (status, tone) = status_of(page.published);
    let message = (form.action == "preview").then_some("Preview updated: not saved yet.");
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "pages"),
            page_header(
                title: &page.title,
                subtitle: Some("Written in Markdown; the storefront shows it once it is published."),
                status_badge(tone: tone, attrs: attributes! { data-page-status=(status) }, (status))
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Pages"</a>
            )
            notice(error: error, message: message)
            page_screen(
                slug: &slug,
                page: &page,
                title: &form.title,
                body: &body,
                preview: &preview,
                media_url: &media_url,
                can_manage: true,
            )
        )
    })
}

/// A whole document around a page's HTML, for the preview frame: the title
/// as the storefront heads it, and plain typography under it.
fn preview_document(title: &str, html: &str) -> String {
    let title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><meta charset=\"utf-8\"><style>\
         body{{font:16px/1.6 system-ui,sans-serif;max-width:46rem;margin:1.5rem auto;padding:0 1rem;color:#1d1d1f;background:#fff}}\
         img{{max-width:100%;height:auto}}table{{border-collapse:collapse}}\
         th,td{{border:1px solid #d2d2d7;padding:.35rem .6rem;text-align:left}}\
         a{{color:#0066cc}}</style><h1>{title}</h1>{html}"
    )
}

/// The short guide under the content field.
#[component]
pub async fn markdown_hint() -> Result<impl View> {
    Ok(view! {
        field_description(
            attrs: attributes! { id="page-body-hint" },
            "Markdown: " <code>"## Heading"</code> ", " <code>"**bold**"</code> ", " <code>"- item"</code> ", "
            <code>"[link](/pages/other)"</code> ". A picture of the page is written by its file name: "
            <code>"![What it shows](01J….webp)"</code> ". HTML is shown as text."
        )
    })
}

/// The editor beside its preview, and the forms that change the rest of the
/// page.
#[component]
async fn page_screen(
    slug: &str,
    page: &PageState,
    title: &str,
    body: &str,
    preview: &str,
    media_url: &str,
    can_manage: bool,
) -> Result<impl View> {
    let id = page.id.as_str();
    let footer_position = page
        .footer_position
        .map(|p| p.to_string())
        .unwrap_or_default();
    let images_full = page.images.len() >= MAX_IMAGES_PER_PAGE;
    Ok(view! {
        <div class="mb-6 grid gap-6 lg:grid-cols-2">
            section_card(
                title: Some("Content"),
                attrs: attributes! { class="mb-0" },
                if can_manage {
                    <form method="post" action=(href!(submit, Slug(slug.to_owned()), PageId(id.to_owned()))) class="flex flex-col gap-4" data-page-form="">
                        field(
                            field_label(attrs: attributes! { for="page-title" }, "Title")
                            input(attrs: attributes! { id="page-title" name="title" required="" maxlength=(MAX_TITLE_LEN) value=(title) })
                        )
                        field(
                            field_label(attrs: attributes! { for="page-body" }, "Content (Markdown)")
                            markdown_hint()
                            textarea(
                                attrs: attributes! {
                                    id="page-body" name="body" rows="24" maxlength=(MAX_BODY_LEN) spellcheck="true"
                                    aria-describedby="page-body-hint" class="font-mono text-xs"
                                },
                                (body)
                            )
                        )
                        <div class="flex flex-wrap gap-2">
                            button(attrs: attributes! { type="submit" name="action" value="save" }, "Save")
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" name="action" value="preview" formnovalidate="" }, "Preview")
                        </div>
                    </form>
                } else {
                    <pre class="max-h-[36rem] overflow-auto whitespace-pre-wrap rounded-lg border border-border bg-muted p-3 font-mono text-xs">(body)</pre>
                }
            )
            section_card(
                title: Some("Preview"),
                attrs: attributes! { class="mb-0" },
                <iframe sandbox="" title="Preview of the page" srcdoc=(preview) data-page-preview=""
                    class="h-[36rem] w-full rounded-lg border border-border bg-white"></iframe>
            )
        </div>

        if can_manage {
            <section class="mb-6 grid gap-6 lg:grid-cols-2">
                <div class=(class!(CARD, "gap-4 px-6"))>
                    <h2 class="text-base font-semibold">"Publication"</h2>
                    if page.published {
                        <p class="text-sm text-muted-foreground">"Shown on the storefront at " <code>"/pages/" (&page.slug)</code> "."</p>
                        <form method="post" action=(href!(unpublish::unpublish, Slug(slug.to_owned()), PageId(id.to_owned())))>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Unpublish")
                        </form>
                    } else {
                        <p class="text-sm text-muted-foreground">"A draft: only the back office sees it."</p>
                        <form method="post" action=(href!(publish::publish, Slug(slug.to_owned()), PageId(id.to_owned())))>
                            button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Publish")
                        </form>
                    }
                    <p class="text-xs text-muted-foreground">"Created " (format_time(page.created_at)) ", updated " (format_time(page.updated_at)) "."</p>
                </div>

                <form method="post" action=(href!(footer::footer, Slug(slug.to_owned()), PageId(id.to_owned()))) class=(class!(CARD, "gap-4 px-6"))>
                    <h2 class="text-base font-semibold">"Footer"</h2>
                    label(
                        checkbox(attrs: attributes! { name="in_footer" value="on" checked=(page.footer_position.is_some()) })
                        "List it in the storefront's footer"
                    )
                    field(
                        field_label(attrs: attributes! { for="page-footer-position" }, "Position (lowest first)")
                        input(attrs: attributes! {
                            id="page-footer-position" name="position" type="number" min="0" max=(MAX_FOOTER_POSITION)
                            value=(&footer_position) placeholder="0"
                        })
                    )
                    <p class="text-sm text-muted-foreground">"A draft is listed once it is published."</p>
                    <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save footer")</div>
                </form>

                <form method="post" action=(href!(seo::seo, Slug(slug.to_owned()), PageId(id.to_owned()))) class=(class!(CARD, "gap-4 px-6"))>
                    <h2 class="text-base font-semibold">"Search engines"</h2>
                    field(
                        field_label(attrs: attributes! { for="page-meta-title" }, "Title")
                        field_description(attrs: attributes! { id="page-meta-title-hint" }, "Empty uses the page's title.")
                        input(attrs: attributes! {
                            id="page-meta-title" name="meta_title" maxlength=(MAX_META_TITLE_LEN)
                            aria-describedby="page-meta-title-hint" value=(&page.meta_title)
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="page-meta-description" }, "Description")
                        field_description(attrs: attributes! { id="page-meta-description-hint" }, "Empty uses the start of the content.")
                        textarea(
                            attrs: attributes! {
                                id="page-meta-description" name="meta_description" rows="3" maxlength=(MAX_META_DESCRIPTION_LEN)
                                aria-describedby="page-meta-description-hint"
                            },
                            (&page.meta_description)
                        )
                    )
                    <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save for search engines")</div>
                </form>

                <form method="post" action=(href!(rename::rename, Slug(slug.to_owned()), PageId(id.to_owned()))) class=(class!(CARD, "gap-4 px-6"))>
                    <h2 class="text-base font-semibold">"Address"</h2>
                    field(
                        field_label(attrs: attributes! { for="page-slug" }, "/pages/…")
                        field_description(attrs: attributes! { id="page-slug-hint" }, "Lowercase letters, digits and dashes. Links to the old address stop working.")
                        input(attrs: attributes! {
                            id="page-slug" name="slug" required="" maxlength=(MAX_SLUG_LEN) aria-describedby="page-slug-hint"
                            pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" value=(&page.slug)
                        })
                    )
                    <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Move")</div>
                </form>
            </section>

            section_card(
                title: Some("Pictures"),
                description: Some("Upload a picture, then copy its line into the content where it should show."),
                if page.images.is_empty() {
                    <p class="text-sm text-muted-foreground">"No picture yet."</p>
                }
                <ul class="divide-y divide-border text-sm">
                    for image in &page.images {
                        <li class="flex items-center gap-3 py-2" data-page-image=(image.file())>
                            <img src=(format!("{media_url}/{}", image.file())) alt="" class="size-12 shrink-0 rounded border border-border object-cover">
                            <code class="min-w-0 flex-1 truncate text-xs" data-page-image-snippet="">"![](" (image.file()) ")"</code>
                            <form method="post" action=(href!(remove_image::remove_image, Slug(slug.to_owned()), PageId(id.to_owned())))>
                                <input type="hidden" name="image_id" value=(&image.id)>
                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove")
                            </form>
                        </li>
                    }
                </ul>
                if images_full {
                    <p class="mt-4 text-sm text-muted-foreground">"This page carries as many pictures as it can. Remove one to add another."</p>
                } else {
                    <form method="post" enctype="multipart/form-data" class="mt-4 flex max-w-xl flex-col gap-4"
                        action=(href!(upload::upload, Slug(slug.to_owned()), PageId(id.to_owned())))>
                        field(
                            field_label(attrs: attributes! { for="page-image" }, "Add a picture")
                            input(attrs: attributes! {
                                id="page-image" name="image" type="file" accept="image/jpeg,image/png,image/webp,image/gif" required=""
                                class="pt-1.5"
                            })
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Upload")</div>
                    </form>
                }
            )

            <form method="post" action=(href!(delete::delete, Slug(slug.to_owned()), PageId(id.to_owned()))) class="flex flex-wrap items-center gap-3">
                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete page")
                <p class="text-sm text-muted-foreground">"For good: its address is free again and its pictures are deleted."</p>
            </form>
        }
    })
}
