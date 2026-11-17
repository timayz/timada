//! `/t/{slug}/articles/{article_id}`: one article, read from its own stream
//! (so an edit shows at once). Members with `pages:manage` write it here
//! beside its preview, and the forms below publish and date it, set what
//! search engines show, move it to another address, add pictures, pick its
//! cover and delete it.
//!
//! The preview is the HTML the storefront shows, already cleaned by
//! [`render_article`]; it still sits in a sandboxed frame with no scripts,
//! as the newsletter's previews do. A product or a category the article
//! shows is drawn there as a box naming it: the cards are the storefront's.

pub mod cover;
pub mod date;
pub mod delete;
pub mod publish;
pub mod remove_image;
pub mod rename;
pub mod seo;
pub mod translations;
pub mod unpublish;
pub mod upload;

use std::sync::Arc;

use timada_core::{Picture, Rendition};

use serde::Deserialize;
use timada_pages::{
    ArticleBlock, ArticleState, MAX_BODY_LEN, MAX_IMAGES_PER_PAGE, MAX_META_DESCRIPTION_LEN,
    MAX_META_TITLE_LEN, MAX_SLUG_LEN, MAX_SUMMARY_LEN, MAX_TITLE_LEN, PageError, render_article,
};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, not_found, see_other},
        href, module_param, page, query_params,
    },
    view::{View, attributes, class, component, view},
};

use super::{articles_url, blog, error_code};
use crate::app::auth::Flash;
use crate::app::t::slug::pages::status_of;
use crate::app::t::slug::{TenantAccess, TenantNav, require_tenant, slug};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::card::CARD;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::textarea::textarea;
use crate::media::Media;
use crate::ui::{
    Shell, document, format_date, format_time, notice, page_header, section_card, status_badge,
};

module_param!(pub article_id);

#[topcoat::router::query_params(error = bad_request)]
struct ArticleQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn article_url(
    cx: &Cx,
    slug: &str,
    article_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, slug: slug.to_owned(), article_id: article_id.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// The article the URL names, or 404.
pub async fn require_article(access: &TenantAccess, cx: &Cx) -> Result<ArticleState> {
    let id = article_id(cx);
    blog(access)?
        .load_article(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// Runs one change of the article the URL names for a member with
/// `pages:manage`, then goes back to it with `message`, or with the code of
/// the refusal.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_pages::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), PageError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let article_id = article_id(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = blog(&access)?;
    match run(&gateway, &access.user_id, &article_id).await {
        Ok(()) => Ok(see_other(article_url(
            cx,
            &slug,
            &article_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(article_url(
            cx,
            &slug,
            &article_id,
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
    let query = query_params::<ArticleQuery>(cx)?;
    let article = require_article(&access, cx).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let nav = TenantNav::of(cx, &access).await?;
    let media_url = context::<Arc<Media>>(cx).tenant_url(&access.tenant.id);
    let preview = preview_document(&article.title, &article.summary, &article.body, &media_url);
    // A shop read in one language has nothing to translate into.
    let translated = blog(&access)?
        .languages()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some_and(|languages| !languages.others.is_empty());
    let tenant = access.tenant;
    let slug = tenant.slug.clone();
    let title = format!("{} · Blog · {}", article.title, tenant.name);
    let back = articles_url(cx, &slug, None, None);
    let (status, tone) = status_of(article.published);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "articles"),
            page_header(
                title: &article.title,
                subtitle: Some("Written in Markdown; the storefront's blog lists it once it is published."),
                status_badge(tone: tone, attrs: attributes! { data-article-status=(status) }, (status))
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Blog"</a>
                if translated {
                    <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(href!(translations::index, slug: slug.clone(), article_id: article.id.clone())) data-translations-link="">"Translations"</a>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            article_screen(
                slug: &slug,
                article: &article,
                title: &article.title,
                summary: &article.summary,
                body: &article.body,
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
    summary: String,
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
    let article = require_article(&access, cx).await?;
    let body = form.body.replace("\r\n", "\n");
    let mut error = None;
    if form.action == "save" {
        match blog(&access)?
            .edit_article(
                &access.user_id,
                &article.id,
                &form.title,
                &form.summary,
                &body,
            )
            .await
        {
            Ok(()) => {
                return Err(see_other(article_url(
                    cx,
                    &access.tenant.slug,
                    &article.id,
                    None,
                    Some("Article saved."),
                ))
                .into());
            }
            Err(err) if err.is_user_facing() => error = Some(error_code(&err)),
            Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
        }
    }
    let nav = TenantNav::of(cx, &access).await?;
    let media_url = context::<Arc<Media>>(cx).tenant_url(&access.tenant.id);
    let preview = preview_document(&form.title, &form.summary, &body, &media_url);
    let tenant = access.tenant;
    let slug = tenant.slug.clone();
    let title = format!("{} · Blog · {}", article.title, tenant.name);
    let back = articles_url(cx, &slug, None, None);
    let (status, tone) = status_of(article.published);
    let message = (form.action == "preview").then_some("Preview updated: not saved yet.");
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "articles"),
            page_header(
                title: &article.title,
                subtitle: Some("Written in Markdown; the storefront's blog lists it once it is published."),
                status_badge(tone: tone, attrs: attributes! { data-article-status=(status) }, (status))
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Blog"</a>
            )
            notice(error: error, message: message)
            article_screen(
                slug: &slug,
                article: &article,
                title: &form.title,
                summary: &form.summary,
                body: &body,
                preview: &preview,
                media_url: &media_url,
                can_manage: true,
            )
        )
    })
}

/// A whole document around an article's HTML, for the preview frame: the
/// title as the storefront heads it, the summary under it, and plain
/// typography. A product or a category the body shows is a box naming it.
fn preview_document(title: &str, summary: &str, body: &str, media_url: &str) -> String {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut html = String::new();
    for block in render_article(body, media_url).blocks {
        match block {
            ArticleBlock::Html(text) => html.push_str(&text),
            // A slug: lowercase letters, digits and dashes.
            ArticleBlock::Product(slug) => {
                html.push_str(&format!(
                    "<p class=\"shown\">Product: <code>{slug}</code></p>"
                ));
            }
            ArticleBlock::Category(slug) => {
                html.push_str(&format!(
                    "<p class=\"shown\">Category: <code>{slug}</code></p>"
                ));
            }
        }
    }
    let summary = if summary.trim().is_empty() {
        String::new()
    } else {
        format!("<p class=\"lead\">{}</p>", escape(summary))
    };
    format!(
        "<!doctype html><meta charset=\"utf-8\"><style>\
         body{{font:16px/1.6 system-ui,sans-serif;max-width:46rem;margin:1.5rem auto;padding:0 1rem;color:#1d1d1f;background:#fff}}\
         img{{max-width:100%;height:auto}}table{{border-collapse:collapse}}\
         th,td{{border:1px solid #d2d2d7;padding:.35rem .6rem;text-align:left}}\
         a{{color:#0066cc}}.lead{{font-size:1.15rem}}\
         .shown{{border:1px dashed #86868b;padding:.75rem 1rem;text-align:center}}</style>\
         <h1>{}</h1>{summary}{html}",
        escape(title)
    )
}

/// The short guide under the content field.
#[component]
pub async fn markdown_hint(cx: &Cx) -> Result<impl View> {
    Ok(view! {
        field_description(
            attrs: attributes! { id="article-body-hint" },
            "Markdown: " <code>"## Heading"</code> ", " <code>"**bold**"</code> ", " <code>"- item"</code> ", "
            <code>"[link](/c/mugs)"</code> ". A picture of the article is written by its file name: "
            <code>"![What it shows](01J….webp)"</code> ". A line of its own shows a product or a category of the shop by its address: "
            <code>"[[product:blue-mug]]"</code> ", " <code>"[[category:mugs]]"</code> ". HTML is shown as text."
        )
    })
}

/// The editor beside its preview, and the forms that change the rest of the
/// article.
#[component]
async fn article_screen(
    cx: &Cx,
    slug: &str,
    article: &ArticleState,
    title: &str,
    summary: &str,
    body: &str,
    preview: &str,
    media_url: &str,
    can_manage: bool,
) -> Result<impl View> {
    let id = article.id.as_str();
    let images_full = article.images.len() >= MAX_IMAGES_PER_PAGE;
    let today = format_date(timada_core::unix_now());
    let cover = article.cover.as_deref();
    Ok(view! {
        <div class="mb-6 grid gap-6 lg:grid-cols-2">
            section_card(
                title: Some("Content"),
                attrs: attributes! { class="mb-0" },
                if can_manage {
                    <form method="post" action=(href!(submit, slug: slug.to_owned(), article_id: id.to_owned())) class="flex flex-col gap-4" data-article-form="">
                        field(
                            field_label(attrs: attributes! { for="article-title" }, "Title")
                            input(attrs: attributes! { id="article-title" name="title" required="" maxlength=(MAX_TITLE_LEN) value=(title) })
                        )
                        field(
                            field_label(attrs: attributes! { for="article-summary" }, "Summary (optional)")
                            field_description(attrs: attributes! { id="article-summary-hint" }, "A sentence or two the blog's list shows under the title. Empty uses the start of the content.")
                            textarea(
                                attrs: attributes! {
                                    id="article-summary" name="summary" rows="2" maxlength=(MAX_SUMMARY_LEN)
                                    aria-describedby="article-summary-hint"
                                },
                                (summary)
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="article-body" }, "Content (Markdown)")
                            markdown_hint()
                            textarea(
                                attrs: attributes! {
                                    id="article-body" name="body" rows="24" maxlength=(MAX_BODY_LEN) spellcheck="true"
                                    aria-describedby="article-body-hint" class="font-mono text-xs"
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
                    if !summary.is_empty() {
                        <p class="mb-3 text-sm text-muted-foreground">(summary)</p>
                    }
                    <pre class="max-h-[36rem] overflow-auto whitespace-pre-wrap rounded-lg border border-border bg-muted p-3 font-mono text-xs">(body)</pre>
                }
            )
            section_card(
                title: Some("Preview"),
                attrs: attributes! { class="mb-0" },
                <iframe sandbox="" title="Preview of the article" srcdoc=(preview) data-article-preview=""
                    class="h-[36rem] w-full rounded-lg border border-border bg-white"></iframe>
            )
        </div>

        if can_manage {
            <section class="mb-6 grid gap-6 lg:grid-cols-2">
                <div class=(class!(CARD, "gap-4 px-6"))>
                    <h2 class="text-base font-semibold">"Publication"</h2>
                    if article.published {
                        <p class="text-sm text-muted-foreground">"On the storefront's blog, at " <code>"/blog/" (&article.slug)</code> "."</p>
                        <form method="post" action=(href!(unpublish::unpublish, slug: slug.to_owned(), article_id: id.to_owned()))>
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Unpublish")
                        </form>
                    } else {
                        <p class="text-sm text-muted-foreground">"A draft: only the back office sees it."</p>
                        <form method="post" action=(href!(publish::publish, slug: slug.to_owned(), article_id: id.to_owned()))>
                            button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Publish")
                        </form>
                    }
                    if article.published_at > 0 {
                        <form method="post" action=(href!(date::date, slug: slug.to_owned(), article_id: id.to_owned())) class="flex flex-col gap-3">
                            field(
                                field_label(attrs: attributes! { for="article-date" }, "Date")
                                field_description(attrs: attributes! { id="article-date-hint" }, "The day the article says it came out, which the blog sorts by: today or before.")
                                input(attrs: attributes! {
                                    id="article-date" name="date" type="date" required="" max=(&today)
                                    aria-describedby="article-date-hint" value=(format_date(article.published_at))
                                })
                            )
                            <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Change the date")</div>
                        </form>
                    }
                    <p class="text-xs text-muted-foreground">"Created " (format_time(article.created_at)) ", updated " (format_time(article.updated_at)) "."</p>
                </div>

                <form method="post" action=(href!(seo::seo, slug: slug.to_owned(), article_id: id.to_owned())) class=(class!(CARD, "gap-4 px-6"))>
                    <h2 class="text-base font-semibold">"Search engines"</h2>
                    field(
                        field_label(attrs: attributes! { for="article-meta-title" }, "Title")
                        field_description(attrs: attributes! { id="article-meta-title-hint" }, "Empty uses the article's title.")
                        input(attrs: attributes! {
                            id="article-meta-title" name="meta_title" maxlength=(MAX_META_TITLE_LEN)
                            aria-describedby="article-meta-title-hint" value=(&article.meta_title)
                        })
                    )
                    field(
                        field_label(attrs: attributes! { for="article-meta-description" }, "Description")
                        field_description(attrs: attributes! { id="article-meta-description-hint" }, "Empty uses the summary, or the start of the content.")
                        textarea(
                            attrs: attributes! {
                                id="article-meta-description" name="meta_description" rows="3" maxlength=(MAX_META_DESCRIPTION_LEN)
                                aria-describedby="article-meta-description-hint"
                            },
                            (&article.meta_description)
                        )
                    )
                    <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save for search engines")</div>
                </form>

                <form method="post" action=(href!(rename::rename, slug: slug.to_owned(), article_id: id.to_owned())) class=(class!(CARD, "gap-4 px-6"))>
                    <h2 class="text-base font-semibold">"Address"</h2>
                    field(
                        field_label(attrs: attributes! { for="article-slug" }, "Address")
                        field_description(attrs: attributes! { id="article-slug-hint" }, "The end of /blog/…: lowercase letters, digits and dashes. The old address redirects to the new one.")
                        input(attrs: attributes! {
                            id="article-slug" name="slug" required="" maxlength=(MAX_SLUG_LEN) aria-describedby="article-slug-hint"
                            pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" value=(&article.slug)
                        })
                    )
                    <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Move")</div>
                </form>
            </section>

            section_card(
                title: Some("Pictures"),
                description: Some("Upload a picture, then copy its line into the content where it should show, or make it the cover the blog's list and a shared link show."),
                if article.images.is_empty() {
                    <p class="text-sm text-muted-foreground">"No picture yet."</p>
                }
                <ul class="divide-y divide-border text-sm">
                    for image in &article.images {
                        <li class="flex items-center gap-3 py-2" data-article-image=(image.file())>
                            <img src=(Picture::url(media_url, &image.id, Rendition::Listing)) alt="" class="size-12 shrink-0 rounded border border-border object-cover">
                            <code class="min-w-0 flex-1 truncate text-xs" data-article-image-snippet="">"![](" (image.file()) ")"</code>
                            <form method="post" action=(href!(cover::cover, slug: slug.to_owned(), article_id: id.to_owned()))>
                                if cover == Some(image.id.as_str()) {
                                    <input type="hidden" name="image_id" value="">
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" data-article-cover="" }, "Cover: take it off")
                                } else {
                                    <input type="hidden" name="image_id" value=(&image.id)>
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Use as cover")
                                }
                            </form>
                            <form method="post" action=(href!(remove_image::remove_image, slug: slug.to_owned(), article_id: id.to_owned()))>
                                <input type="hidden" name="image_id" value=(&image.id)>
                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="h-7 px-2 text-xs" }, "Remove")
                            </form>
                        </li>
                    }
                </ul>
                if images_full {
                    <p class="mt-4 text-sm text-muted-foreground">"This article carries as many pictures as it can. Remove one to add another."</p>
                } else {
                    <form method="post" enctype="multipart/form-data" class="mt-4 flex max-w-xl flex-col gap-4"
                        action=(href!(upload::upload, slug: slug.to_owned(), article_id: id.to_owned()))>
                        field(
                            field_label(attrs: attributes! { for="article-image" }, "Add a picture")
                            input(attrs: attributes! {
                                id="article-image" name="image" type="file" accept="image/jpeg,image/png,image/webp,image/gif" required=""
                                class="pt-1.5"
                            })
                        )
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Upload")</div>
                    </form>
                }
            )

            <form method="post" action=(href!(delete::delete, slug: slug.to_owned(), article_id: id.to_owned())) class="flex flex-wrap items-center gap-3">
                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Delete article")
                <p class="text-sm text-muted-foreground">"For good: its address is free again and its pictures are deleted."</p>
            </form>
        }
    })
}
