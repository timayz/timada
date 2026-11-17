//! `/t/{slug}/languages`: the languages the storefront is read in, for
//! members with `pages:read`; those with `pages:manage` set them. The main
//! one is what the products, the categories, the pages and the articles are
//! written in; each of the others has its translations beside them. Read
//! from the tenant's own stream, so a change shows at once here; the
//! storefront follows a moment later.

pub mod set;

use timada_core::languages::LANGUAGES;
use timada_pages::MAX_OTHER_LANGUAGES;
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::announcements::pages;
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::components::button::button;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_description, field_label};
use crate::components::select::select;
use crate::ui::{Shell, document, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct LanguagesQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn languages_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<LanguagesQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let languages = pages(&access)?
        .languages()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Languages · {}", tenant.name);
    let state = if languages.is_some() { "set" } else { "none" };
    let main = languages
        .as_ref()
        .map(|languages| languages.main.clone())
        .unwrap_or_default();
    let others = languages
        .map(|languages| languages.others)
        .unwrap_or_default();
    let limit = format!(
        "Up to {MAX_OTHER_LANGUAGES}. A language taken off keeps its translations for the day it comes back."
    );

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "languages"),
            page_header(
                title: "Languages",
                subtitle: Some("The languages your storefront is read in. Your products, categories, pages and articles are written in the main one, and translated into the others; where a translation is missing, a visitor reads the main wording."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Storefront languages"),
                <form method="post" action=(href!(set::set, slug: slug.clone())) class="flex flex-col gap-4" data-shop-languages=(state)>
                    <fieldset class="contents" disabled=(!can_manage)>
                        field(
                            attrs: attributes! { class="max-w-xs" },
                            field_label(attrs: attributes! { for="shop-main-language" }, "Main language")
                            field_description(attrs: attributes! { id="shop-main-language-hint" }, "The one everything is written in today. Its addresses carry no language; the others' start with theirs, as in " <code>"/en/…"</code> ".")
                            select(
                                attrs: attributes! { id="shop-main-language" name="main" aria-describedby="shop-main-language-hint" required="" },
                                if state == "none" {
                                    <option value="" selected="">"Choose…"</option>
                                }
                                for (code, name) in LANGUAGES {
                                    <option value=(code) selected=(*code == main)>(name)</option>
                                }
                            )
                        )
                        <fieldset class="min-w-0" aria-describedby="shop-other-languages-hint">
                            <legend class="text-sm font-medium">"Translated into"</legend>
                            <p id="shop-other-languages-hint" class="mt-1 text-sm text-muted-foreground">(limit)</p>
                            <div class="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 sm:grid-cols-3 md:grid-cols-4">
                                for (code, name) in LANGUAGES {
                                    <label class="flex items-center gap-2 py-0.5 text-sm" data-language-option=(code)>
                                        checkbox(attrs: attributes! {
                                            name=(format!("{}{code}", set::LANGUAGE_FIELD_PREFIX)) value="on"
                                            checked=(others.iter().any(|other| other == code))
                                        })
                                        (name)
                                    </label>
                                }
                            </div>
                        </fieldset>
                        if can_manage {
                            <div>button(attrs: attributes! { type="submit" }, "Save the languages")</div>
                        }
                    </fieldset>
                </form>
            )
        )
    })
}
