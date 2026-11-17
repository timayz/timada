//! `/t/{slug}/articles/{article_id}/translations`: what the article says in the shop's
//! other languages, a card each, read from the translations' own streams.

pub mod remove;
pub mod save;

use timada_pages::{PageError, TextKind};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        href, page, query_params,
    },
    view::{View, view},
};

use super::{article_id, article_url, require_article};
use crate::app::auth::Flash;
use crate::app::t::slug::announcements::{error_code, pages};
use crate::app::t::slug::{TenantNav, require_tenant, slug};
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::ui::translations::{MainText, text_cards_of, text_translation_cards};
use crate::ui::{LINK, Shell, document, notice, page_header};

#[topcoat::router::query_params(error = bad_request)]
struct TranslationsQuery {
    error: Option<String>,
    message: Option<String>,
}

fn translations_url(
    cx: &Cx,
    slug: &str,
    id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, slug: slug.to_owned(), article_id: id.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// What both actions share: the member may manage pages, and the outcome
/// goes back to this page.
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
    let id = article_id(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = pages(&access)?;
    match run(&gateway, &access.user_id, &id).await {
        Ok(()) => Ok(see_other(translations_url(
            cx,
            &slug,
            &id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(translations_url(
            cx,
            &slug,
            &id,
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
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let query = query_params::<TranslationsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let text = require_article(&access, cx).await?;
    let gateway = pages(&access)?;
    let languages = gateway
        .languages()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .map(|languages| languages.others)
        .unwrap_or_default();
    let translations = gateway
        .load_text_translations(TextKind::Article, &text.id, &languages)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let cards = text_cards_of(&languages, &translations);
    let main = MainText {
        title: text.title.clone(),
        body: text.body.clone(),
        meta_title: text.meta_title.clone(),
        meta_description: text.meta_description.clone(),
        summary: Some(text.summary.clone()),
    };
    let tenant = access.tenant;
    let slug = tenant.slug.clone();
    let back = article_url(cx, &slug, &text.id, None, None);
    let languages_page =
        href!(crate::app::t::slug::languages::index, slug: slug.clone()).resolve(cx);
    let save = href!(save::save, slug: slug.clone(), article_id: text.id.clone()).resolve(cx);
    let remove = href!(remove::remove, slug: slug.clone(), article_id: text.id.clone()).resolve(cx);
    let title = format!("Translations · {} · {}", text.title, tenant.name);
    let heading = format!("Translations of {}", text.title);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "articles"),
            page_header(
                title: &heading,
                subtitle: Some("What this article says in each of the shop's other languages. It is shown there once the article itself is published."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"Back to the article"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if cards.is_empty() {
                <p class="text-sm text-muted-foreground" data-no-languages="">
                    "Your storefront is read in one language. "
                    <a class=(LINK) href=(&languages_page)>"Add the languages"</a>
                    " it is translated into first."
                </p>
            }
            text_translation_cards(editable: can_manage, save: &save, remove: &remove, main: &main, cards: &cards)
        )
    })
}
