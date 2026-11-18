//! `/t/{slug}/newsletter-popup`: the pop-up that invites a visitor of the
//! storefront to its newsletter, for members with `pages:read`; those with
//! `pages:manage` set it. What it offers, how long a visitor is on a page
//! before it opens, and whether it also opens as they leave. Read from the
//! tenant's own stream, so a change shows at once here; the storefront
//! follows a moment later, opens it once for a visitor and never for one
//! who subscribed.

pub mod set;
pub mod wording;

use timada_pages::{MAX_POPUP_DELAY_SECS, MAX_POPUP_TEXT_LEN, MAX_POPUP_TITLE_LEN};
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
use crate::components::input::input;
use crate::components::textarea::textarea;
use crate::ui::{Shell, document, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct PopupQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn popup_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<PopupQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let popup = pages(&access)?
        .newsletter_popup()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    // What each of the shop's other languages reads on it: the language,
    // its card's heading, the title and the text there.
    let other_languages = pages(&access)?
        .languages()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .map(|languages| languages.others)
        .unwrap_or_default();
    let mut worded_in: Vec<(String, String, String, String)> = Vec::new();
    for language in other_languages {
        let worded = pages(&access)?
            .shop_wording(&language)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        let heading = format!(
            "The pop-up in {}",
            timada_core::languages::language_name(&language).unwrap_or(&language)
        );
        worded_in.push((language, heading, worded.popup_title, worded.popup_text));
    }
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Newsletter pop-up · {}", tenant.name);
    let state = if popup.shown { "shown" } else { "off" };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter_popup"),
            page_header(
                title: "Newsletter pop-up",
                subtitle: Some("Invites a visitor of the storefront to your newsletter, once. Whoever signs up is mailed a link to confirm, as from the footer's form."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("The pop-up"),
                <form method="post" action=(href!(set::set, slug: slug.clone())) class="flex max-w-2xl flex-col gap-4" data-newsletter-popup=(state)>
                    <fieldset class="contents" disabled=(!can_manage)>
                        <label class="flex items-center gap-2 text-sm font-medium">
                            checkbox(attrs: attributes! { name="shown" value="on" checked=(popup.shown) })
                            "Show the pop-up on the storefront"
                        </label>
                        field(
                            field_label(attrs: attributes! { for="popup-title" }, "What it offers")
                            field_description(attrs: attributes! { id="popup-title-hint" }, "A few words, as its heading: a discount on a first order, a buying guide. What you promise is yours to send: an automation started by a confirmed subscription mails a code or a link.")
                            input(attrs: attributes! {
                                id="popup-title" name="title" type="text" maxlength=(MAX_POPUP_TITLE_LEN)
                                value=(&popup.title) aria-describedby="popup-title-hint"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="popup-text" }, "A sentence under it (optional)")
                            textarea(
                                attrs: attributes! { id="popup-text" name="text" rows="3" maxlength=(MAX_POPUP_TEXT_LEN) },
                                (&popup.text)
                            )
                        )
                        field(
                            field_label(attrs: attributes! { for="popup-delay" }, "Opens after (seconds on a page)")
                            input(attrs: attributes! {
                                id="popup-delay" name="delay_secs" type="number" min="0" max=(MAX_POPUP_DELAY_SECS)
                                required="" value=(popup.delay_secs)
                            })
                        )
                        <div>
                            <label class="flex items-center gap-2 text-sm font-medium">
                                checkbox(attrs: attributes! {
                                    name="on_leave" value="on" checked=(popup.on_leave)
                                    aria-describedby="popup-leave-hint"
                                })
                                "Also open it as the visitor leaves"
                            </label>
                            <p id="popup-leave-hint" class="mt-1 text-sm text-muted-foreground">
                                "When the pointer goes up out of the page, towards the tabs or the address bar, before the delay is over. A phone has no pointer: there, only the delay counts."
                            </p>
                        </div>
                        if can_manage {
                            <div>button(attrs: attributes! { type="submit" }, "Save the pop-up")</div>
                        }
                    </fieldset>
                </form>
            )

            for (language, heading, their_title, their_text) in &worded_in {
                section_card(
                    title: Some(heading),
                    description: Some("What a visitor reading this language sees. What is left empty is read in the main language."),
                    attrs: attributes! { data-popup-wording=(language) },
                    <form method="post" action=(href!(wording::wording, slug: slug.clone())) class="flex max-w-2xl flex-col gap-4">
                        <input type="hidden" name="language" value=(language) />
                        <fieldset class="contents" disabled=(!can_manage)>
                            field(
                                field_label(attrs: attributes! { for=(format!("popup-title-{language}")) }, "Translated offer")
                                input(attrs: attributes! {
                                    id=(format!("popup-title-{language}")) name="title" type="text" maxlength=(MAX_POPUP_TITLE_LEN)
                                    value=(their_title) placeholder=(&popup.title) lang=(language)
                                })
                            )
                            field(
                                field_label(attrs: attributes! { for=(format!("popup-text-{language}")) }, "Translated sentence under it")
                                textarea(
                                    attrs: attributes! { id=(format!("popup-text-{language}")) name="text" rows="3" maxlength=(MAX_POPUP_TEXT_LEN) placeholder=(&popup.text) lang=(language) },
                                    (their_text)
                                )
                            )
                            if can_manage {
                                <div>button(attrs: attributes! { type="submit" }, "Save translation")</div>
                            }
                        </fieldset>
                    </form>
                )
            }

            section_card(
                title: Some("When it opens"),
                <ul class="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
                    <li>"Once for a visitor: closed or answered, it does not come back."</li>
                    <li>"Never for a visitor who gave their address to the newsletter, here or in the footer, nor for a shopper who subscribed from their account."</li>
                    <li>"Not on the cart, the checkout or the account, and not while the cookie banner still waits for an answer."</li>
                    <li>"An address is added to the list only once its owner clicks the link mailed to them."</li>
                </ul>
            )
        )
    })
}
