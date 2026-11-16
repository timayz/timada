//! `/t/{slug}/google-tag`: the Google tag the storefront writes into its
//! pages, for members with `pages:read`; those with `pages:manage` set it
//! up. The Analytics and Google Ads ids, the labels of the conversions a
//! sale, an item added to the cart and a checkout begun are counted under,
//! and whether a sale also carries the buyer's hashed details. Read from
//! the tenant's own stream, so a change shows at once here; the storefront
//! follows a moment later, and holds the tag back as each visitor's answer
//! to its cookie banner says.

pub mod set;

use timada_pages::MAX_GOOGLE_ID_LEN;
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
use crate::ui::{Shell, document, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct GoogleTagQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn google_tag_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<GoogleTagQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let tag = pages(&access)?
        .google_tag()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Google tag · {}", tenant.name);
    let state = if tag.is_set() { "set" } else { "none" };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "google_tag"),
            page_header(
                title: "Google tag",
                subtitle: Some("Counts your sales for Google Analytics and Google Ads. The storefront writes the tag into its pages and holds it back until each visitor answers its cookie banner."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Tag and conversions"),
                <form method="post" action=(href!(set::set, slug: slug.clone())) class="flex max-w-2xl flex-col gap-4" data-google-tag=(state)>
                    <fieldset class="contents" disabled=(!can_manage)>
                        field(
                            field_label(attrs: attributes! { for="tag-analytics-id" }, "Analytics measurement ID")
                            field_description(attrs: attributes! { id="tag-analytics-id-hint" }, "From Google Analytics, under Admin, Data streams: " <code>"G-XXXXXXXXXX"</code> ". Leave empty to count for Google Ads only.")
                            input(attrs: attributes! {
                                id="tag-analytics-id" name="analytics_id" type="text" maxlength=(MAX_GOOGLE_ID_LEN)
                                value=(tag.analytics_id.as_deref().unwrap_or_default())
                                aria-describedby="tag-analytics-id-hint" placeholder="G-XXXXXXXXXX" autocomplete="off"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="tag-ads-id" }, "Google Ads conversion ID")
                            field_description(attrs: attributes! { id="tag-ads-id-hint" }, "From Google Ads, under Goals, Conversions, in the tag's setup: " <code>"AW-123456789"</code> ". The labels below belong to it.")
                            input(attrs: attributes! {
                                id="tag-ads-id" name="ads_id" type="text" maxlength=(MAX_GOOGLE_ID_LEN)
                                value=(tag.ads_id.as_deref().unwrap_or_default())
                                aria-describedby="tag-ads-id-hint" placeholder="AW-123456789" autocomplete="off"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="tag-purchase-label" }, "Label of the sale conversion")
                            field_description(attrs: attributes! { id="tag-purchase-label-hint" }, "The part after the slash of the conversion action's " <code>"send_to"</code> ", or the whole of it. Sent once per order from its confirmation page, with the order's total, its currency and its number. Make this one the primary conversion.")
                            input(attrs: attributes! {
                                id="tag-purchase-label" name="purchase_label" type="text" maxlength="80"
                                value=(tag.purchase_label.as_deref().unwrap_or_default())
                                aria-describedby="tag-purchase-label-hint" autocomplete="off"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="tag-add-to-cart-label" }, "Label of the add-to-cart conversion")
                            field_description(attrs: attributes! { id="tag-add-to-cart-label-hint" }, "Optional, a secondary conversion: sent when an item goes into the cart.")
                            input(attrs: attributes! {
                                id="tag-add-to-cart-label" name="add_to_cart_label" type="text" maxlength="80"
                                value=(tag.add_to_cart_label.as_deref().unwrap_or_default())
                                aria-describedby="tag-add-to-cart-label-hint" autocomplete="off"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="tag-begin-checkout-label" }, "Label of the checkout conversion")
                            field_description(attrs: attributes! { id="tag-begin-checkout-label-hint" }, "Optional, a secondary conversion: sent when the checkout opens.")
                            input(attrs: attributes! {
                                id="tag-begin-checkout-label" name="begin_checkout_label" type="text" maxlength="80"
                                value=(tag.begin_checkout_label.as_deref().unwrap_or_default())
                                aria-describedby="tag-begin-checkout-label-hint" autocomplete="off"
                            })
                        )
                        <div>
                            <label class="flex items-center gap-2 text-sm font-medium">
                                checkbox(attrs: attributes! {
                                    name="enhanced_conversions" value="on" checked=(tag.enhanced_conversions)
                                    aria-describedby="tag-enhanced-hint"
                                })
                                "Enhanced conversions"
                            </label>
                            <p id="tag-enhanced-hint" class="mt-1 text-sm text-muted-foreground">
                                "A sale also tells Google Ads the buyer's e-mail address, phone and name, hashed, and their address, so it can match the sale to a click when cookies cannot. Only for a buyer who allowed advertising cookies. Turn it on in the conversion action too, and say so in your privacy policy."
                            </p>
                        </div>
                        if can_manage {
                            <div>button(attrs: attributes! { type="submit" }, "Save the tag")</div>
                        }
                    </fieldset>
                </form>
            )

            section_card(
                title: Some("What is sent"),
                <ul class="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
                    <li><code>"purchase"</code>": the order's total as the shopper paid it, its currency, its lines and its number as the transaction."</li>
                    <li><code>"add_to_cart"</code>" and "<code>"begin_checkout"</code>": the items and what they cost."</li>
                    <li>"Nothing before a visitor answers the cookie banner, unless the storefront runs Google's advanced consent mode, where the tag loads without cookies."</li>
                    <li>"Empty every field and save to take the tag off the storefront."</li>
                </ul>
            )
        )
    })
}
