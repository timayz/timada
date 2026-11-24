//! `/t/{slug}/feed`: the shop's shopping feed, for members with
//! `products:read`; those with `products:manage` open it, move it to a new
//! address and close it. Whether it is open is read from the tenant's own
//! stream, so a change shows at once; the address itself answers a moment
//! later, once the read model has followed.

pub mod ads_label;
pub mod close;
pub mod open;

use timada_catalog::{FEED_LABELS, OUT_OF_ADS_LABEL, count_feed_products};
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
use crate::components::select::select;
use crate::config::Config;
use crate::db::Db;
use crate::ui::{LINK, Shell, document, fact, facts, notice, page_header, section_card};

#[topcoat::router::query_params(error = bad_request)]
struct FeedQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The feeds of one country and language the shop sells in.
struct MarketFeeds {
    /// What they are for: `BE · French`.
    label: String,
    /// What their addresses name the market by: `be-fr`.
    code: String,
    /// A reader's name, then the address it fetches.
    addresses: [(&'static str, String); 3],
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
    let ads_label = catalog(&access)?
        .load_feed_ads_label()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .map(usize::from);
    let ads_hint = format!(
        "The products whose main category is kept out of ads carry « {OUT_OF_ADS_LABEL} » in this label, in place of their own: exclude that value in your campaigns. A category is kept out on its own page."
    );
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
    let server = context::<Config>(cx)
        .server
        .url
        .as_str()
        .trim_end_matches('/')
        .to_owned();
    let address = token.as_ref().map(|token| {
        format!(
            "{server}{}",
            href!(feeds::google::google, feeds::token: token.clone()).resolve(cx)
        )
    });
    // The same products for the other catalogues, and the reviews, under
    // the same secret: (what it is for, its field, its address, its hint).
    let others: Vec<(&str, &str, String, &str)> = token
        .as_ref()
        .map(|token| {
            vec![
                (
                    "Meta catalogue address",
                    "meta",
                    format!(
                        "{server}{}",
                        href!(feeds::meta::meta, feeds::token: token.clone()).resolve(cx)
                    ),
                    "For catalogue ads and retargeting on Facebook and Instagram: in Commerce Manager, add it to a catalogue as a scheduled data feed. Its item ids are the ones the storefront's Meta pixel sends.",
                ),
                (
                    "TikTok catalogue address",
                    "tiktok",
                    format!(
                        "{server}{}",
                        href!(feeds::tiktok::tiktok, feeds::token: token.clone()).resolve(cx)
                    ),
                    "For TikTok catalogue ads: in TikTok Ads Manager, add it to a catalogue as a scheduled data feed.",
                ),
                (
                    "Product reviews address",
                    "google-reviews",
                    format!(
                        "{server}{}",
                        href!(feeds::google_reviews::google_reviews, feeds::token: token.clone())
                            .resolve(cx)
                    ),
                    "The shop's published reviews, so Shopping listings show their stars: in Merchant Center, add it as a product reviews feed. Google asks for at least 50 reviews in all before it shows ratings.",
                ),
            ]
        })
        .unwrap_or_default();
    // A feed per country and language the shop sells in: what it is for,
    // its code, then the address each reader fetches.
    let mut markets: Vec<MarketFeeds> = Vec::new();
    if let Some(token) = &token {
        let sold_in = crate::feed::markets_of(access.executor()?, &access.tenant)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        for market in sold_in {
            let code = market.code();
            let language =
                timada_core::languages::language_name(&market.language).unwrap_or(&market.language);
            let at = |path: String| format!("{server}{path}");
            markets.push(MarketFeeds {
                label: format!("{} · {language}", market.country),
                code: code.clone(),
                addresses: [
                    (
                        "Google",
                        at(href!(feeds::market::google::google, feeds::token: token.clone(), feeds::market::market: code.clone()).resolve(cx)),
                    ),
                    (
                        "Meta",
                        at(href!(feeds::market::meta::meta, feeds::token: token.clone(), feeds::market::market: code.clone()).resolve(cx)),
                    ),
                    (
                        "TikTok",
                        at(href!(feeds::market::tiktok::tiktok, feeds::token: token.clone(), feeds::market::market: code.clone()).resolve(cx)),
                    ),
                ],
            });
        }
    }
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Shopping feed · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "feed"),
            page_header(
                title: "Shopping feed",
                subtitle: Some("The shop's products as Google Merchant Center, Meta and TikTok read them, for Shopping ads, free listings and catalogue ads, and its reviews for the stars on Shopping listings."),
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
                title: Some("Out of ads"),
                description: Some("Some categories are worth a page and not a campaign: a click on what they are searched by costs more than a sale can pay."),
                attrs: attributes! { data-feed-ads-label=(ads_label.map(|label| label.to_string()).unwrap_or_default()) },
                <form method="post" action=(href!(ads_label::ads_label, slug: slug.clone())) class="flex max-w-3xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="feed-ads-label" }, "Label that marks them")
                        field_description(attrs: attributes! { id="feed-ads-label-hint" }, (&ads_hint))
                        select(
                            attrs: attributes! { id="feed-ads-label" name="label" aria-describedby="feed-ads-label-hint" disabled=(!can_manage) class="w-48" },
                            <option value="" selected=(ads_label.is_none())>"None"</option>
                            for n in 0..FEED_LABELS {
                                <option value=(n) selected=(ads_label == Some(n))>"Custom label " (n)</option>
                            }
                        )
                    )
                    if can_manage {
                        <div>button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save the label")</div>
                    }
                </form>
            )

            section_card(
                title: Some("Address"),
                description: Some("Give the first to Merchant Center as a scheduled fetch (a file in XML), and each of the others to its own reader. Whoever has an address reads the feed, so they are not guessable: move the feed to new ones if one got out, and the old ones stop answering."),
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
                    for (label, key, other, hint) in &others {
                        field(
                            attrs: attributes! { class="mt-4 max-w-3xl" },
                            field_label(attrs: attributes! { for=(format!("feed-address-{key}")) }, (*label))
                            field_description(attrs: attributes! { id=(format!("feed-address-{key}-hint")) }, (*hint))
                            input(attrs: attributes! {
                                id=(format!("feed-address-{key}")) type="text" readonly="" value=(other)
                                aria-describedby=(format!("feed-address-{key}-hint")) data-feed-other=(*key)
                            })
                        )
                    }
                } else {
                    <p class="text-sm text-muted-foreground">"The feed is closed: it has no address."</p>
                }
                if !markets.is_empty() {
                    <h3 class="mt-6 text-sm font-medium">"Per country and language"</h3>
                    <p class="mt-1 max-w-3xl text-sm text-muted-foreground">"One feed for each country your shipping methods name, in each language of the shop: that country's delivery prices, and each product worded and linked as that language reads it. A product with no translation is listed in the main language. Give each to the reader's feed for that country and language."</p>
                    <ul class="mt-3 flex max-w-3xl flex-col gap-4" data-feed-markets=(markets.len())>
                        for MarketFeeds { label, code, addresses } in &markets {
                            <li data-feed-market=(code)>
                                <p class="text-sm font-medium">(label)</p>
                                for (reader, address) in addresses {
                                    <label class="mt-2 block text-xs text-muted-foreground" for=(format!("feed-{code}-{}", reader.to_lowercase()))>(*reader) " feed for " (label)</label>
                                    input(attrs: attributes! {
                                        id=(format!("feed-{code}-{}", reader.to_lowercase())) type="text" readonly="" value=(address)
                                        data-feed-market-address=(reader.to_lowercase())
                                    })
                                }
                            </li>
                        }
                    </ul>
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
