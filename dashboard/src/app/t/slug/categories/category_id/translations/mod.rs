//! `/t/{slug}/categories/{category_id}/translations`: what the category
//! says in the shop's other languages, a card each, read from the
//! translations' own streams.

pub mod remove;
pub mod save;

use timada_catalog::{MAX_CATEGORY_DESCRIPTION_LEN, MAX_CATEGORY_NAME_LEN, TranslationKind};
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        href, page, query_params,
    },
    view::{View, view},
};

use super::{category_id, category_url, require_category};
use crate::app::auth::Flash;
use crate::app::t::slug::announcements::pages;
use crate::app::t::slug::categories::error_code;
use crate::app::t::slug::products::catalog;
use crate::app::t::slug::{TenantAccess, TenantNav, require_tenant, slug};
use crate::components::breadcrumb::{
    breadcrumb, breadcrumb_item, breadcrumb_link, breadcrumb_list, breadcrumb_page,
    breadcrumb_separator,
};
use crate::ui::translations::{
    MainWording, questions_text, translation_cards, translation_cards_of,
};
use crate::ui::{LINK, Shell, document, notice, page_header};

#[topcoat::router::query_params(error = bad_request)]
struct TranslationsQuery {
    error: Option<String>,
    message: Option<String>,
}

fn translations_url(
    cx: &Cx,
    slug: &str,
    category_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(
        index,
        slug: slug.to_owned(),
        category_id: category_id.to_owned()
    )
    .query(Flash { error, message })
    .resolve(cx)
}

/// What both actions share: the member may manage products, and the
/// outcome goes back to this page.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_catalog::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), timada_catalog::CatalogError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let category_id = category_id(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = catalog(&access)?;
    match run(&gateway, &access.user_id, &category_id).await {
        Ok(()) => Ok(see_other(translations_url(
            cx,
            &slug,
            &category_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(translations_url(
            cx,
            &slug,
            &category_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<TranslationsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let category = require_category(cx, &access).await?;
    let languages = pages(&access)?
        .languages()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .map(|languages| languages.others)
        .unwrap_or_default();
    let translations = catalog(&access)?
        .load_translations(TranslationKind::Category, &category.id, &languages)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let cards = translation_cards_of(&languages, &translations);
    let main = MainWording {
        name: category.name.clone(),
        description: category.description.clone(),
        meta_title: category.meta_title.clone(),
        meta_description: category.meta_description.clone(),
        name_max: MAX_CATEGORY_NAME_LEN,
        description_max: MAX_CATEGORY_DESCRIPTION_LEN,
        category: Some((category.body.clone(), questions_text(&category.questions))),
        images: Vec::new(),
    };
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let editable = can_manage && !category.archived;
    let back = category_url(cx, &slug, &category.id, None, None);
    let languages_page =
        href!(crate::app::t::slug::languages::index, slug: slug.clone()).resolve(cx);
    let save = href!(save::save, slug: slug.clone(), category_id: category.id.clone()).resolve(cx);
    let remove =
        href!(remove::remove, slug: slug.clone(), category_id: category.id.clone()).resolve(cx);
    let title = format!("Translations · {} · {}", category.name, tenant.name);
    let heading = format!("Translations of {}", category.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "categories"),
            breadcrumb(
                attrs: topcoat::view::attributes! { class="mb-4" },
                breadcrumb_list(
                    breadcrumb_item(breadcrumb_link(attrs: topcoat::view::attributes! { href=(&back) }, (&category.name)))
                    breadcrumb_separator()
                    breadcrumb_item(breadcrumb_page("Translations"))
                )
            )
            page_header(
                title: &heading,
                subtitle: Some("What this category says in each of the shop's other languages."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if cards.is_empty() {
                <p class="text-sm text-muted-foreground" data-no-languages="">
                    "Your storefront is read in one language. "
                    <a class=(LINK) href=(&languages_page)>"Add the languages"</a>
                    " it is translated into first."
                </p>
            }
            translation_cards(editable: editable, save: &save, remove: &remove, main: &main, cards: &cards)
        )
    })
}
