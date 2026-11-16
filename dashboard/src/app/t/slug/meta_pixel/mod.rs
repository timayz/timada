//! `/t/{slug}/meta-pixel`: the Meta pixel the storefront writes into its
//! pages, for members with `pages:read`; those with `pages:manage` set it
//! up. The pixel's id, and the Conversions API token under which each sale
//! is also told to Meta from the server: the token is sealed before it is
//! recorded and never shown again. Read from the tenant's own stream, so a
//! change shows at once here; the storefront follows a moment later, and
//! writes the pixel only for the visitors who allowed advertising.

pub mod set;

use timada_pages::MAX_META_PIXEL_ID_LEN;
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::announcements::pages;
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::ConversionsSealing;
use crate::app::auth::Flash;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::button;
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::ui::{Shell, document, notice, page_header, section_card};

/// The longest Conversions API token taken.
pub const MAX_TOKEN_LEN: usize = 600;

#[topcoat::router::query_params(error = bad_request)]
struct MetaPixelQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn meta_pixel_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<MetaPixelQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let pixel = pages(&access)?
        .load_meta_pixel()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let pixel_id = pixel
        .as_ref()
        .and_then(|pixel| pixel.pixel_id.clone())
        .unwrap_or_default();
    let has_token = pixel
        .as_ref()
        .is_some_and(|pixel| pixel.token_sealed.is_some());
    let refusal = pixel.and_then(|pixel| pixel.refusal);
    // Without the operator's secret no token can be kept.
    let can_seal = context::<ConversionsSealing>(cx).0.is_some();
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Meta pixel · {}", tenant.name);
    let state = match (pixel_id.is_empty(), has_token) {
        (true, _) => "none",
        (false, false) => "pixel",
        (false, true) => "token",
    };

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "meta_pixel"),
            page_header(
                title: "Meta pixel",
                subtitle: Some("Lets Meta measure your ads on Facebook and Instagram and show them again to your visitors. The storefront writes the pixel into its pages for the visitors who allowed advertising cookies, and for no one else."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if let Some(refusal) = &refusal {
                alert(
                    variant: AlertVariant::Destructive,
                    attrs: attributes! { role="alert" class="mb-4" data-meta-refusal="" },
                    alert_description("Meta refused the last sale sent with this token: " (refusal) " Check the token and the pixel it was made for, then save the token again.")
                )
            }

            section_card(
                title: Some("Pixel and server events"),
                <form method="post" action=(href!(set::set, slug: slug.clone())) class="flex max-w-2xl flex-col gap-4" data-meta-pixel=(state)>
                    <fieldset class="contents" disabled=(!can_manage)>
                        field(
                            field_label(attrs: attributes! { for="meta-pixel-id" }, "Pixel ID")
                            field_description(attrs: attributes! { id="meta-pixel-id-hint" }, "From Meta Events Manager, under Data sources: the digits below the pixel's name. Leave empty to take the pixel off the storefront.")
                            input(attrs: attributes! {
                                id="meta-pixel-id" name="pixel_id" type="text" inputmode="numeric" maxlength=(MAX_META_PIXEL_ID_LEN)
                                value=(&pixel_id)
                                aria-describedby="meta-pixel-id-hint" autocomplete="off"
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="meta-token" }, "Conversions API token")
                            field_description(attrs: attributes! { id="meta-token-hint" },
                                if !can_seal {
                                    "Not available on this installation: its operator has not set the key the tokens are kept under."
                                } else if has_token {
                                    "A token is kept. It is never shown again: leave this empty to keep it, or paste a new one to replace it."
                                } else {
                                    "Optional. From Events Manager, in the pixel's settings, under Conversions API: generate an access token and paste it here. Each sale is then also sent to Meta from the server, so it is counted when the browser's event is lost."
                                }
                            )
                            input(attrs: attributes! {
                                id="meta-token" name="token" type="password" maxlength=(MAX_TOKEN_LEN)
                                aria-describedby="meta-token-hint" autocomplete="off" disabled=(!can_seal)
                            })
                        )
                        if has_token {
                            <label class="flex items-center gap-2 text-sm font-medium">
                                checkbox(attrs: attributes! { name="remove_token" value="on" })
                                "Take the token off"
                            </label>
                        }
                        if can_manage {
                            <div>button(attrs: attributes! { type="submit" }, "Save the pixel")</div>
                        }
                    </fieldset>
                </form>
            )

            section_card(
                title: Some("What is sent"),
                <ul class="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
                    <li><code>"PageView"</code>" on every page, "<code>"ViewContent"</code>" on a product, "<code>"AddToCart"</code>", "<code>"InitiateCheckout"</code>" and "<code>"Purchase"</code>", once per order, with the order's total."</li>
                    <li>"Nothing for a visitor who did not allow advertising cookies: the pixel is not written into their pages."</li>
                    <li>"With a token, the sale of a shopper who allowed advertising is also sent from the server, under the same event ID, so Meta counts it once. It carries the buyer's e-mail address, phone, name, city, postal code and country, hashed, and their browser's Meta identifiers. Say so in your privacy policy."</li>
                </ul>
            )
        )
    })
}
