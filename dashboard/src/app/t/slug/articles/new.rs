//! `/t/{slug}/articles/new`, for members with `pages:manage`: an article
//! written as a draft. Saving it opens its own screen, where it is
//! published; a refused one comes back with what was typed.

use serde::Deserialize;
use timada_pages::{CreateArticle, MAX_BODY_LEN, MAX_SLUG_LEN, MAX_SUMMARY_LEN, MAX_TITLE_LEN};
use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::see_other, href, page},
    view::{View, attributes, component, view},
};

use super::article_id::{article_url, markdown_hint};
use super::{articles_url, blog, error_code};
use crate::app::t::slug::{TenantNav, require_tenant, slug};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::textarea::textarea;
use crate::ui::{Shell, document, notice, page_header, section_card};

/// What the new article form posts.
#[derive(Deserialize, Default)]
pub struct NewArticleForm {
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    body: String,
}

#[page]
pub async fn new(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let title = format!("New article · Blog · {}", tenant.name);
    let back = articles_url(cx, &tenant.slug, None, None);
    let form = NewArticleForm::default();
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "articles"),
            page_header(
                title: "New article",
                subtitle: Some("Saved as a draft: nobody sees it until you publish it."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Blog"</a>
            )
            new_article_form(slug: &tenant.slug, form: &form)
        )
    })
}

#[page(POST)]
pub async fn create(cx: &Cx, Form(form): Form<NewArticleForm>) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let created = blog(&access)?
        .create_article(
            &access.user_id,
            CreateArticle {
                slug: form.slug.clone(),
                title: form.title.clone(),
                summary: form.summary.clone(),
                body: form.body.clone(),
            },
        )
        .await;
    let error = match created {
        Ok(id) => {
            return Err(see_other(article_url(
                cx,
                &access.tenant.slug,
                &id,
                None,
                Some("Draft saved."),
            ))
            .into());
        }
        Err(err) if err.is_user_facing() => error_code(&err),
        Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
    };
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let title = format!("New article · Blog · {}", tenant.name);
    let back = articles_url(cx, &tenant.slug, None, None);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "articles"),
            page_header(
                title: "New article",
                subtitle: Some("Saved as a draft: nobody sees it until you publish it."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Blog"</a>
            )
            notice(error: Some(error))
            new_article_form(slug: &tenant.slug, form: &form)
        )
    })
}

#[component]
async fn new_article_form(cx: &Cx, slug: &str, form: &NewArticleForm) -> Result<impl View> {
    Ok(view! {
        section_card(
            <form method="post" action=(href!(create, slug: slug.to_owned())) class="flex max-w-3xl flex-col gap-4" data-article-form="">
                field(
                    field_label(attrs: attributes! { for="article-title" }, "Title")
                    input(attrs: attributes! { id="article-title" name="title" required="" maxlength=(MAX_TITLE_LEN) value=(&form.title) })
                )
                field(
                    field_label(attrs: attributes! { for="article-slug" }, "Address (optional)")
                    field_description(attrs: attributes! { id="article-slug-hint" }, "The end of /blog/…: lowercase letters, digits and dashes. Leave it empty to derive it from the title.")
                    input(attrs: attributes! {
                        id="article-slug" name="slug" maxlength=(MAX_SLUG_LEN) aria-describedby="article-slug-hint"
                        pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" placeholder="bien-choisir-son-tapis" value=(&form.slug)
                    })
                )
                field(
                    field_label(attrs: attributes! { for="article-summary" }, "Summary (optional)")
                    field_description(attrs: attributes! { id="article-summary-hint" }, "A sentence or two the blog's list shows under the title. Empty uses the start of the content.")
                    textarea(
                        attrs: attributes! {
                            id="article-summary" name="summary" rows="2" maxlength=(MAX_SUMMARY_LEN)
                            aria-describedby="article-summary-hint"
                        },
                        (&form.summary)
                    )
                )
                field(
                    field_label(attrs: attributes! { for="article-body" }, "Content (Markdown)")
                    markdown_hint()
                    textarea(
                        attrs: attributes! {
                            id="article-body" name="body" rows="20" maxlength=(MAX_BODY_LEN) spellcheck="true"
                            aria-describedby="article-body-hint" class="font-mono text-xs"
                        },
                        (&form.body)
                    )
                )
                <div>button(attrs: attributes! { type="submit" }, "Save draft")</div>
            </form>
        )
    })
}
