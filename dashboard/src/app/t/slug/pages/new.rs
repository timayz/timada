//! `/t/{slug}/pages/new`, for members with `pages:manage`: a page written as
//! a draft. Saving it opens its own screen, where it is published; a refused
//! one comes back with what was typed.

use serde::Deserialize;
use timada_pages::{CreatePage, MAX_BODY_LEN, MAX_SLUG_LEN, MAX_TITLE_LEN};
use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::see_other, href, page},
    view::{View, attributes, component, view},
};

use super::page_id::{markdown_hint, page_url};
use super::{error_code, pages, pages_url};
use crate::app::t::slug::{Slug, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::textarea::textarea;
use crate::ui::{Shell, document, notice, page_header, section_card};

/// What the new page form posts.
#[derive(Deserialize, Default)]
pub struct NewPageForm {
    title: String,
    #[serde(default)]
    slug: String,
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
    let title = format!("New page · Pages · {}", tenant.name);
    let back = pages_url(cx, &tenant.slug, None, None);
    let form = NewPageForm::default();
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "pages"),
            page_header(
                title: "New page",
                subtitle: Some("Saved as a draft: nobody sees it until you publish it."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Pages"</a>
            )
            new_page_form(slug: &tenant.slug, form: &form)
        )
    })
}

#[page(POST)]
pub async fn create(cx: &Cx, Form(form): Form<NewPageForm>) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let created = pages(&access)?
        .create_page(
            &access.user_id,
            CreatePage {
                slug: form.slug.clone(),
                title: form.title.clone(),
                body: form.body.clone(),
            },
        )
        .await;
    let error = match created {
        Ok(id) => {
            return Err(see_other(page_url(
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
    let title = format!("New page · Pages · {}", tenant.name);
    let back = pages_url(cx, &tenant.slug, None, None);
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "pages"),
            page_header(
                title: "New page",
                subtitle: Some("Saved as a draft: nobody sees it until you publish it."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to Pages"</a>
            )
            notice(error: Some(error))
            new_page_form(slug: &tenant.slug, form: &form)
        )
    })
}

#[component]
async fn new_page_form(slug: &str, form: &NewPageForm) -> Result<impl View> {
    Ok(view! {
        section_card(
            <form method="post" action=(href!(create, Slug(slug.to_owned()))) class="flex max-w-3xl flex-col gap-4" data-page-form="">
                field(
                    field_label(attrs: attributes! { for="page-title" }, "Title")
                    input(attrs: attributes! { id="page-title" name="title" required="" maxlength=(MAX_TITLE_LEN) value=(&form.title) })
                )
                field(
                    field_label(attrs: attributes! { for="page-slug" }, "Address (optional)")
                    field_description(attrs: attributes! { id="page-slug-hint" }, "The end of /pages/…: lowercase letters, digits and dashes. Leave it empty to derive it from the title.")
                    input(attrs: attributes! {
                        id="page-slug" name="slug" maxlength=(MAX_SLUG_LEN) aria-describedby="page-slug-hint"
                        pattern="[a-z0-9][a-z0-9-]*[a-z0-9]" placeholder="mentions-legales" value=(&form.slug)
                    })
                )
                field(
                    field_label(attrs: attributes! { for="page-body" }, "Content (Markdown)")
                    markdown_hint()
                    textarea(
                        attrs: attributes! {
                            id="page-body" name="body" rows="20" maxlength=(MAX_BODY_LEN) spellcheck="true"
                            aria-describedby="page-body-hint" class="font-mono text-xs"
                        },
                        (&form.body)
                    )
                )
                <div>button(attrs: attributes! { type="submit" }, "Save draft")</div>
            </form>
        )
    })
}
