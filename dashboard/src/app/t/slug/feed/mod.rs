//! `/t/{slug}/feed`: the shop's shopping feed, for members with
//! `products:read`; those with `products:manage` open it, move it to a new
//! address and close it. Whether it is open is read from the tenant's own
//! stream, so a change shows at once; the address itself answers a moment
//! later, once the read model has followed.

pub mod close;
pub mod open;

use timada_catalog::count_feed_products;
use timada_tenancy::{PERMISSION_PRODUCTS_MANAGE, PERMISSION_PRODUCTS_READ};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::products::catalog;
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::app::feeds::token as feeds;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::config::Config;
use crate::db::Db;
use crate::ui::{LINK, Shell, document, fact, facts, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct FeedQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn feed_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_PRODUCTS_MANAGE).await?;
    let query = query_params::<FeedQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let token = catalog(&access)?
        .load_feed_token()
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let has_storefront = super::orders::orders(&access)?
        .storefront_address()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some();
    let carried = count_feed_products(context::<Db>(cx), &access.tenant.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let carried_label = match carried {
        1 => "1 product".to_owned(),
        n => format!("{n} products"),
    };
    // The address a reader fetches: this server's own, as it is reached
    // from outside.
    let address = token.as_ref().map(|token| {
        format!(
            "{}{}",
            context::<Config>(cx)
                .server
                .url
                .as_str()
                .trim_end_matches('/'),
            href!(feeds::google::google, feeds::token: token.clone()).resolve(cx)
        )
    });
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Shopping feed · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "feed"),
            page_header(
                title: "Shopping feed",
                subtitle: Some("The shop's products as Google Merchant Center reads them, for Shopping ads and free listings."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("What it carries"),
                description: Some("One item per variant on sale of every published product, with its price, pictures, stock, delivery prices and what the product's Shopping feed tab says of it. A product can be taken out there and stay in the shop."),
                facts(
                    fact(term: "State", attrs: attributes! { data-feed-state=(if address.is_some() { "open" } else { "closed" }) }, if address.is_some() { "Open" } else { "Closed" })
                    fact(term: "In the feed", attrs: attributes! { data-feed-products=(carried) }, (&carried_label))
                )
                if !has_storefront {
                    <p class="mt-4 text-sm text-destructive-text" data-feed-needs-storefront="">
                        "The feed links each item to its page in the shop, and the shop's address is not set: the feed answers nothing until it is. Set it under "
                        <a class=(LINK) href=(href!(super::api_keys::index, slug: slug.clone()))>"API keys"</a> "."
                    </p>
                }
            )

            section_card(
                title: Some("Address"),
                description: Some("Give it to Merchant Center as a scheduled fetch (a file in XML). Whoever has the address reads the feed, so it is not guessable: move the feed to a new one if this one got out, and the old one stops answering."),
                if let Some(address) = &address {
                    field(
                        attrs: attributes! { class="max-w-3xl" },
                        field_label(attrs: attributes! { for="feed-address" }, "Feed address")
                        field_description(attrs: attributes! { id="feed-address-hint" }, "A change in the catalog reaches it within moments.")
                        input(attrs: attributes! {
                            id="feed-address" type="text" readonly="" value=(address)
                            aria-describedby="feed-address-hint" data-feed-address=""
                        })
                    )
                } else {
                    <p class="text-sm text-muted-foreground">"The feed is closed: it has no address."</p>
                }
                if can_manage {
                    <div class="mt-4 flex flex-wrap gap-2">
                        <form method="post" action=(href!(open::open, slug: slug.clone()))>
                            if address.is_some() {
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Move to a new address")
                            } else {
                                button(attrs: attributes! { type="submit" }, "Open the feed")
                            }
                        </form>
                        if address.is_some() {
                            <form method="post" action=(href!(close::close, slug: slug.clone()))>
                                button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Close the feed")
                            </form>
                        }
                    </div>
                }
            )
        )
    })
}
