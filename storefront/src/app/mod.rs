//! The pages: the home at `/`, the whole catalogue at `/boutique`, a category
//! at `/c/{slug}`, a product at `/p/{slug}`, the cart at `/cart`, the checkout
//! at `/checkout`, a placed order at `/orders/{order_id}`, the shopper's
//! account at `/account`, the support page at `/support-client`, the shop's
//! own pages at their own addresses (`/mentions-legales`, …), its blog at
//! `/blog` and `/blog/{slug}`, the cookie
//! banner's choices at `/cookies`, `/robots.txt` and `/sitemap.xml` for search
//! engines, and the library's `/store/status`. One stylesheet (`theme.css`)
//! inlined in every page, layout only, and no asset bundle.

pub mod account;
pub mod address;
pub mod blog;
pub mod boutique;
pub mod c;
pub mod cart;
pub mod checkout;
pub mod conditions_generales_de_vente;
pub mod cookies;
pub mod emails;
pub mod filters;
pub mod indisponible;
pub mod lang;
pub mod mentions_legales;
pub mod menu;
pub mod moved;
pub mod newsletter;
pub mod orders;
pub mod own_page;
pub mod p;
pub mod politique_de_confidentialite;
pub mod politique_de_livraison;
pub mod politique_de_remboursement;
pub mod popup;
pub mod qui_sommes_nous;
pub mod reviews;
pub mod search;
pub mod sitemap;
pub mod support_client;
pub mod tag;
pub mod topbar;
pub mod words;

use timada_storefront::{
    CategoryNode, ConsentMode, PIXEL_EVENTS_SCRIPT, Product, ProductQuery, ProductSort,
    RouterBuilderStoreExt, SocialCard, Store, TAG_EVENTS_SCRIPT, TIME_ZONE_SCRIPT, consent_script,
    google_tag_head, is_store_unavailable, json_ld, menu, meta_pixel_head, organization_ld,
    page_view_event, pixel_add_to_cart_event, pixel_event_id, product_image, products,
    recent_reviews, tag_events_json, website_ld,
};
use topcoat::{
    Result,
    context::{Cx, try_context},
    cookie::{CookieLayer, Key},
    router::{
        Body, BodyLimit, IpNet, LayerFn, LayerFuture, Method, Next, Router,
        RouterBuilderDiscoverExt, TrustedProxies, href, module_router, page,
        request::{method, original_uri},
    },
    view::{Child, NodeViewParts, PartsWriter, Unescaped, View, attributes, view},
};

use crate::shopper::accounts_enabled;
use cart::drawer::{CART_SCRIPT, DRAWER_CSS, cart_drawer};
use cookies::consent_banner;
use indisponible::{StoreAway, UnavailablePage};
use menu::{MENU_CSS, MENU_SCRIPT, site_menu};
use search::{SEARCH_CSS, SEARCH_SCRIPT};
use topbar::announcement_bar;
use words::{Worded, t};

/// A review carries up to three photos of up to 8 MiB each.
const REVIEW_LIMIT: usize = 26 * 1024 * 1024;

/// The shop's name, as the pages say it.
pub const SHOP_NAME: &str = "Ma boutique";
/// The address the support page and the footer name.
pub const CONTACT_EMAIL: &str = "contact@example.com";

/// What the shop's tags do before a visitor answers the cookie banner:
/// nothing at all ([`ConsentMode::Basic`]), or load with consent denied and
/// send Google pings without cookies, which it models conversions from
/// ([`ConsentMode::Advanced`]).
pub const CONSENT_MODE: ConsentMode = ConsentMode::Basic;

/// The layout, inlined in every page. Add your look to this sheet.
const THEME_CSS: &str = include_str!("theme.css");

/// `trusted_proxies`: the proxies in front of the app, whose
/// `X-Forwarded-For` names the shopper; the dashboard limits the sign-in
/// codes each shopper's address may ask for.
pub fn router(store: Store, cookie_key: Key, trusted_proxies: Vec<IpNet>) -> Router {
    module_router!()
        .discover()
        .layer(UnavailablePage)
        .layer(address::Addresses)
        .layer(moved::MovedAddress)
        .layer(LayerFn::new(Some("/orders"), pass).with(BodyLimit::max(REVIEW_LIMIT)))
        // At `/` and before the cookie layer, which then runs outside it.
        .layer(LayerFn::new(Some("/"), remember_source))
        // Outside the layers above: they and the pages see the French
        // address, and what they answer is turned back on its way out.
        .layer(lang::Localized)
        .trusted_proxies(TrustedProxies::new().networks(trusted_proxies))
        .layer(CookieLayer::new())
        .with(cookie_key)
        .store(store)
        .build()
}

/// Lets a request through, for a layer that only carries route context.
fn pass<'a>(cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
    next.run(cx, body)
}

/// Keeps what brought the visitor (a click id, a campaign, another site),
/// when the address of a page says so and they allowed it. The checkout's
/// pages are left out: a bank's or Stripe's page sending the shopper back is
/// not what brought them.
fn remember_source<'a>(cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
    Box::pin(async move {
        let path = lang::speaking(cx).await.internal;
        if *method(cx) == Method::GET
            && !path.starts_with("/checkout")
            && let Err(err) = crate::shopper::remember_request(cx)
        {
            // A sale is never lost to what measures it.
            tracing::warn!(error = %err, "cannot keep what brought the visitor");
        }
        next.run(cx, body).await
    })
}

/// An amount as a reader of `language` expects it: `1 234,50€` in French
/// (`12,50 USD` for another currency), `€1,234.50` in English
/// (`12.50 USD`), which every other language is written as until it has a
/// way of its own.
pub fn write_money(language: &str, cents: u64, currency: &str) -> String {
    let french = language == "fr";
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(if french { '\u{a0}' } else { ',' });
        }
        grouped.push(digit);
    }
    let cents = cents % 100;
    match (french, currency) {
        (true, "EUR") => format!("{grouped},{cents:02}€"),
        (true, other) => format!("{grouped},{cents:02}\u{a0}{other}"),
        (false, "EUR") => format!("€{grouped}.{cents:02}"),
        (false, other) => format!("{grouped}.{cents:02}\u{a0}{other}"),
    }
}

/// A rate in basis points as a reader of `language` expects it: `20`,
/// `5,5` in French, `5.5` in English.
pub fn write_rate(language: &str, rate_bps: u32) -> String {
    let rate = format!("{:.2}", f64::from(rate_bps) / 100.0);
    let rate = rate.trim_end_matches('0').trim_end_matches('.');
    if language == "fr" {
        rate.replace('.', ",")
    } else {
        rate.to_owned()
    }
}

/// `19,90€` or `€19.90`: an amount as the shop writes its prices in the
/// language of the request, which [`Localized`](lang::Localized) carries
/// as the library's `MoneyFormat`, so a page's own prices and the
/// library's totals read alike. A view shows it as is; anywhere else,
/// [`Money::resolve`] reads it.
pub fn money(cents: i64, currency: &str) -> Money<'_> {
    Money {
        cents: cents.max(0) as u64,
        currency,
    }
}

/// An amount, written in the visitor's language when it is shown.
#[derive(Clone, Copy)]
pub struct Money<'a> {
    cents: u64,
    currency: &'a str,
}

impl Money<'_> {
    pub fn resolve(&self, cx: &Cx) -> String {
        timada_storefront::money(cx).format(self.cents, self.currency)
    }
}

impl NodeViewParts for Money<'_> {
    fn into_view_parts(self, cx: &Cx, parts: &mut PartsWriter<'_>) {
        parts.push_string(self.resolve(cx));
    }
}

/// How long delivery takes, as a shopper reads it: `8 à 15 jours`, `3 jours`.
pub fn delivery_days(days: timada_storefront::DeliveryDays) -> Worded<'static> {
    if days.min == days.max {
        t!(delivery_days, days: days.max)
    } else {
        t!(delivery_days_range, min: days.min, max: days.max)
    }
}

/// A product's price line: `19,90€`, or `19,90€ – 29,90€` across its variants.
pub fn price_line(cx: &Cx, product: &Product) -> String {
    match product.price_range() {
        Some(range) if range.varies() => format!(
            "{} – {}",
            money(range.min_cents, &range.currency).resolve(cx),
            money(range.max_cents, &range.currency).resolve(cx)
        ),
        Some(range) => money(range.max_cents, &range.currency).resolve(cx),
        None => String::new(),
    }
}

/// The HTML shell every page shares: the announcements bar, the masthead, the
/// category bar, the page, the footer, and (unless `no_drawer`, for the
/// cart and the checkout, which show the cart already) the cart drawer.
#[topcoat::view::component]
pub async fn document(
    cx: &Cx,
    title: &str,
    #[default] nav: &[CategoryNode],
    #[default] no_drawer: bool,
    /// What search engines show under the title.
    #[default]
    description: Option<&str>,
    /// Keeps the page out of search engines: a listing narrowed or sorted,
    /// which shows the products of its plain address again. The pages under
    /// [`PRIVATE_PATHS`](sitemap::PRIVATE_PATHS) are kept out whatever this
    /// says.
    #[default]
    noindex: bool,
    /// The address search engines keep for the page, as a path: its own
    /// unless said otherwise (a filtered listing names the plain one, a
    /// variant's address the product's).
    #[default]
    canonical: Option<&str>,
    /// The path of the page in the languages that read it at another one
    /// than `canonical`'s: a product, a category and an article go by a
    /// slug of their own in each (see [`lang::Speaking::elsewhere`]).
    #[default]
    elsewhere: &[(&'static str, String)],
    /// A picture a shared link shows, as a whole address.
    #[default]
    image: Option<&str>,
    /// What the page sells, in minor units of a currency, for a shared
    /// link.
    #[default]
    price: Option<(i64, &str)>,
    /// When the page's article came out (Unix seconds), for a shared link.
    #[default]
    published: Option<i64>,
    /// What the page shows, as schema.org data.
    #[default]
    structured: &[serde_json::Value],
    /// Leaves the cookie banner out: the page that asks the same question
    /// at length.
    #[default]
    no_banner: bool,
    /// What the page tells the shop's Google tag and Meta pixel (the
    /// product shown, the checkout opened, a sale), as [`tag`] builds it;
    /// an item just added to the cart is told by every page.
    #[default]
    events: Option<&tag::Told>,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    // The page answering an outage asks the dashboard nothing more; any
    // other page shows its cart, or says the cart is away when the
    // dashboard is, and goes on: the catalogue is the shop's own copy.
    let away = try_context::<StoreAway>(cx).is_some();
    let (cart, cart_unavailable) = if away {
        (None, true)
    } else {
        match cart::current_cart(cx).await {
            Ok(cart) => (cart, false),
            Err(err) if is_store_unavailable(&err) => {
                tracing::warn!(error = %err, "the cart cannot be read: the dashboard is away");
                (None, true)
            }
            Err(err) => return Err(err),
        }
    };
    let count = cart
        .as_ref()
        .map(|cart| cart.item_count)
        .unwrap_or_default();
    let total = cart
        .as_ref()
        .map(|cart| {
            money(
                cart.subtotal_cents as i64,
                cart.currency.as_deref().unwrap_or("EUR"),
            )
            .resolve(cx)
        })
        .unwrap_or_else(|| money(0, "EUR").resolve(cx));
    let accounts = accounts_enabled(cx);
    let signed_in = accounts && !away && crate::shopper::signed_in(cx).await.is_some();
    let announcements = timada_storefront::announcements(cx).await?;
    // The newsletter's pop-up, for a visitor it was never put to.
    let invitation = if away {
        None
    } else {
        popup::popup_for(cx).await?
    };
    let consent = crate::shopper::consent(cx);
    // The shop's tag, unless the visitor's answer holds it back, and what
    // this page tells it. The note of an item added is taken either way: a
    // page without the tag tells nobody, now or later.
    let tag = timada_storefront::google_tag(cx).await?;
    let tag_head = tag
        .as_ref()
        .and_then(|tag| google_tag_head(tag, CONSENT_MODE, consent));
    let added = crate::shopper::take_added(cx);
    let mut told = Vec::new();
    if let (Some(tag), Some(_)) = (&tag, &tag_head) {
        if let Some(added) = &added {
            told.extend(tag::added_events(tag, added));
        }
        if let Some(events) = events {
            told.extend_from_slice(&events.tag);
        }
    }
    // The shop's Meta pixel, for a visitor who allowed advertising: every
    // page tells it it was seen, then what it shows. An item added is told
    // apart, for the cart's script to send it alone from a page it fetched.
    let pixel = timada_storefront::meta_pixel(cx).await?;
    let pixel_head = pixel
        .as_deref()
        .and_then(|pixel_id| meta_pixel_head(pixel_id, consent));
    let mut pixel_told = Vec::new();
    let mut pixel_added = Vec::new();
    if pixel_head.is_some() {
        pixel_told.push(page_view_event(&pixel_event_id()));
        if let Some(events) = events {
            pixel_told.extend_from_slice(&events.pixel);
        }
        if let Some(added) = &added {
            pixel_added.push(pixel_add_to_cart_event(
                &added.currency,
                &added.items,
                &pixel_event_id(),
            ));
        }
    }
    let speaking = lang::speaking(cx).await;
    let language = speaking.language;
    let noindex = noindex || sitemap::is_private(&speaking.internal);
    let canonical_path = canonical.unwrap_or(&speaking.internal);
    let canonical = speaking.absolute(cx, canonical_path);
    // The same page in the shop's other languages, for search engines and
    // for the footer's links; a page kept out of them names none.
    let alternates = speaking.alternates(cx, canonical_path, elsewhere);
    let main_address = alternates.first().map(|(_, address)| address.clone());
    let card = SocialCard {
        title,
        description: description.unwrap_or_default(),
        url: &canonical,
        site_name: SHOP_NAME,
        image,
        price,
        published_at: published,
    }
    .tags();
    let page_title = if title == SHOP_NAME {
        t!(shell_home_title, shop: SHOP_NAME, tagline: t!(shop_tagline).resolve(cx))
            .resolve(cx)
            .into_owned()
    } else {
        format!("{title} – {SHOP_NAME}")
    };
    Ok(view! {
        <!DOCTYPE html>
        <html lang=(language)>
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(page_title)</title>
                if let Some(description) = description {
                    <meta name="description" content=(description)>
                }
                if noindex {
                    <meta name="robots" content="noindex">
                }
                // Before any tag: Google's start from what the visitor allowed.
                <script>(Unescaped::new_unchecked(consent_script(CONSENT_MODE, consent)))</script>
                if let Some(head) = &tag_head {
                    <script async="" src=(&head.src) data-google-tag=""></script>
                    <script>(Unescaped::new_unchecked(head.script.clone()))</script>
                }
                if let Some(head) = &pixel_head {
                    <script>(Unescaped::new_unchecked(head.clone()))</script>
                }
                <link rel="canonical" href=(&canonical)>
                if !noindex {
                    for (code, address) in &alternates {
                        <link rel="alternate" hreflang=(*code) href=(address)>
                    }
                    if let Some(address) = &main_address {
                        <link rel="alternate" hreflang="x-default" href=(address)>
                    }
                }
                for tag in &card {
                    if tag.property {
                        <meta property=(tag.key) content=(&tag.content)>
                    } else {
                        <meta name=(tag.key) content=(&tag.content)>
                    }
                }
                for data in structured {
                    <script type="application/ld+json">(Unescaped::new_unchecked(json_ld(data)))</script>
                }
                <style>(Unescaped::new_unchecked(THEME_CSS))</style>
                <style>(Unescaped::new_unchecked(DRAWER_CSS))</style>
                <style>(Unescaped::new_unchecked(MENU_CSS))</style>
                <style>(Unescaped::new_unchecked(SEARCH_CSS))</style>
                if invitation.is_some() {
                    <style>(Unescaped::new_unchecked(popup::POPUP_CSS))</style>
                }
            </head>
            <body>
                if consent.is_none() && !no_banner {
                    consent_banner()
                }
                announcement_bar(announcements: &announcements)
                <header class="masthead">
                    <div class="wrap">
                        if !nav.is_empty() {
                            <a class="menu-toggle" href="#menu" aria-controls="menu">(t!(shell_menu))</a>
                        }
                        <a class="brand" href=(href!(index))>(SHOP_NAME)</a>
                        <form class="search" method="get" action=(href!(boutique::index)) role="search"
                            data-search=(href!(search::index)) data-search-min=(search::MIN_CHARS)>
                            <input name="q" type="search" placeholder=(t!(shell_search.placeholder)) aria-label=(t!(shell_search.label))
                                autocomplete="off" aria-autocomplete="list" aria-controls="search-drop" aria-expanded="false">
                            <button>(t!(shell_search))</button>
                            <div id="search-drop" class="search-drop" data-search-drop="" hidden=""></div>
                        </form>
                        <nav class="quick" aria-label=(t!(shell_quick_label))>
                            <a class="quick-support" href=(href!(support_client::index))>(t!(shell_support))</a>
                            if accounts {
                                <a class="account-link" href=(href!(account::index)) data-account-link="">
                                    if signed_in { (t!(shell_account)) } else { (t!(shell_sign_in)) }
                                </a>
                            }
                            <a class="cart-link" href=(href!(cart::index)) data-cart-link="" aria-controls="cart">
                                <span class="quick-label">(t!(shell_cart))</span>
                                <span class="count" data-cart-count="" hidden=(count == 0)>(count)</span>
                                <span class="cart-total" data-cart-total="" hidden=(cart_unavailable)>(total)</span>
                            </a>
                        </nav>
                    </div>
                </header>
                if !nav.is_empty() {
                    site_menu(nav: nav)
                    <script>(Unescaped::new_unchecked(MENU_SCRIPT))</script>
                }
                <script>(Unescaped::new_unchecked(SEARCH_SCRIPT))</script>
                <script>(Unescaped::new_unchecked(TIME_ZONE_SCRIPT))</script>
                if tag_head.is_some() {
                    // Before the cart's script, which hands it the pages it fetches.
                    <script type="application/json" data-tag-events="">(Unescaped::new_unchecked(tag_events_json(&told)))</script>
                    <script>(Unescaped::new_unchecked(TAG_EVENTS_SCRIPT))</script>
                }
                if pixel_head.is_some() {
                    <script type="application/json" data-pixel-events="">(Unescaped::new_unchecked(tag_events_json(&pixel_told)))</script>
                    <script type="application/json" data-pixel-events="added">(Unescaped::new_unchecked(tag_events_json(&pixel_added)))</script>
                    <script>(Unescaped::new_unchecked(PIXEL_EVENTS_SCRIPT))</script>
                }
                <main>(child)</main>
                site_footer(language: language, alternates: &alternates)
                if let Some(invitation) = &invitation {
                    popup::newsletter_dialog(popup: invitation)
                }
                if !no_drawer {
                    cart_drawer(cart: cart.as_ref(), unavailable: cart_unavailable)
                    <script>(Unescaped::new_unchecked(CART_SCRIPT))</script>
                }
            </body>
        </html>
    })
}

/// The footer: the shop, its legal pages, the useful links and how to
/// reach it. The pages are the shop's own routes; their text comes from the
/// back office.
#[topcoat::view::component]
pub async fn site_footer(
    cx: &Cx,
    /// The language the page is read in.
    language: &str,
    /// The page in each language of the shop, as whole addresses: none for
    /// a shop read in one.
    alternates: &[(&'static str, String)],
) -> Result<impl View> {
    // The blog is linked once it has something to read.
    let blog = timada_storefront::has_articles(cx).await?;
    Ok(view! {
        <footer class="sitefooter">
            <div class="wrap">
                <div>
                    <a class="brand" href=(href!(index))>(SHOP_NAME)</a>
                    <p>(t!(shop_tagline))</p>
                </div>
                <div>
                    <h2>(t!(footer_about))</h2>
                    <ul>
                        <li><a href=(href!(mentions_legales::index))>(t!(footer_legal_notice))</a></li>
                        <li><a href=(href!(conditions_generales_de_vente::index))>(t!(footer_terms_of_sale))</a></li>
                        <li><a href=(href!(politique_de_confidentialite::index))>(t!(footer_privacy_policy))</a></li>
                        <li><a href=(href!(politique_de_remboursement::index))>(t!(footer_refund_policy))</a></li>
                        <li><a href=(href!(politique_de_livraison::index))>(t!(footer_shipping_policy))</a></li>
                        <li><a href=(href!(cookies::index))>(t!(footer_cookies))</a></li>
                    </ul>
                </div>
                <div>
                    <h2>(t!(footer_useful_links))</h2>
                    <ul>
                        <li><a href=(href!(account::index))>(t!(footer_track_order))</a></li>
                        <li><a href=(href!(boutique::index))>(t!(footer_products))</a></li>
                        <li><a href=(href!(support_client::index))>(t!(shell_support))</a></li>
                        <li><a href=(href!(qui_sommes_nous::index))>(t!(footer_about_us))</a></li>
                        if blog {
                            <li><a href=(href!(blog::index)) data-blog-link="">(t!(footer_blog))</a></li>
                        }
                    </ul>
                </div>
                <div>
                    <h2>(t!(footer_contact))</h2>
                    <p><a href=(format!("mailto:{CONTACT_EMAIL}"))>(CONTACT_EMAIL)</a></p>
                </div>
                <div>
                    <h2>(t!(footer_newsletter))</h2>
                    <p>(t!(footer_newsletter_pitch))</p>
                    newsletter::newsletter_form()
                </div>
            </div>
            if !alternates.is_empty() {
                // `hreflang` first: it tells [`lang::Localized`] the link
                // goes to another language's address on purpose.
                <nav class="languages" aria-label=(t!(footer_languages)) data-languages="">
                    <ul>
                        for (code, address) in alternates {
                            <li>
                                <a hreflang=(*code) href=(address) lang=(*code) aria-current=((*code == language).then_some("true"))>
                                    (lang::native_name(code))
                                </a>
                            </li>
                        }
                    </ul>
                </nav>
            }
            <p class="legal">"© " (SHOP_NAME)</p>
        </footer>
    })
}

/// A product's card, as the listings show it: the picture, the name, the
/// price (a range across the variants) and the button: a product sold as
/// is goes straight into the cart, one with options sends to its page.
///
/// On a category's page (`under`, its id) the card links the product
/// under that category; anywhere else, under its main one.
#[topcoat::view::component]
pub async fn product_card(
    cx: &Cx,
    product: &Product,
    #[default] under: Option<&str>,
) -> Result<impl View> {
    let paths = address::paths(cx).await?;
    let link = match under {
        Some(category_id) => address::product_href_under(cx, paths, category_id, product),
        None => address::product_href(cx, paths, product),
    };
    let varies = product.price_range().is_some_and(|range| range.varies());
    let simple = product.options.is_empty();
    // Where the cart drawer opens after the add: this very page.
    let here = original_uri(cx)
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "/".to_owned());
    Ok(view! {
        <li class="card" data-product=(&product.slug)>
            <a class="card-pic" href=(&link)>
                if let Some(image) = product.image() {
                    product_image(image: image, attrs: attributes! { class="thumb" })
                }
            </a>
            <div class="card-body">
                <h3 class="card-title"><a href=(&link)>(&product.name)</a></h3>
                <p class="card-price">
                    if varies { <small>(t!(card_price_range))</small> }
                    if let (Some(was), Some(currency)) = (product.compare_at_cents, product.currency.as_deref()) {
                        if product.variants.is_empty() {
                            <s class="was" data-compare-at=""><span class="unseen">(t!(price_former)) " "</span>(money(was, currency))</s>
                        }
                    }
                    <span data-price="">(price_line(cx, product))</span>
                </p>
                if let (Some(rating), Some(whole)) = (product.rating(), product.rating_stars()) {
                    <p class="card-rating" data-rating=(format!("{rating:.1}"))>
                        <span class="stars" aria-hidden="true">(timada_storefront::stars(whole))</span>
                        " " (t!(card_review_count, count: product.review_count))
                    </p>
                }
                <div class="card-act">
                    if simple {
                        <form method="post" action=(href!(cart::add::add))>
                            <input type="hidden" name="slug" value=(&product.slug)>
                            <input type="hidden" name="quantity" value="1">
                            <input type="hidden" name="from" value=(here)>
                            <button class="btn">(t!(card_add_to_cart))</button>
                        </form>
                    } else {
                        <a class="btn" href=(&link)>(t!(card_choose_options))</a>
                    }
                </div>
            </div>
        </li>
    })
}

/// A grid of product cards: the catalogue, a category and a search share it.
/// A category's page names itself (`under`), for the cards to link under it.
#[topcoat::view::component]
pub async fn catalog_grid(
    cx: &Cx,
    products: &[Product],
    #[default] under: Option<&str>,
) -> Result<impl View> {
    Ok(view! {
        <ul class="catalog">
            for product in products {
                product_card(product: product, under: under)
            }
        </ul>
    })
}

/// How many products the home's rails show.
const NEW_ARRIVALS: u32 = 8;
const PICKS: u32 = 6;
const TILES: usize = 8;
/// How many of a category's newest products are looked at for its tile.
const TILE_CANDIDATES: u32 = 8;
const QUOTES: u32 = 3;

/// A category's tile: its name, its count, and the picture of a product in it.
struct Tile {
    node: CategoryNode,
    image: Option<timada_storefront::Image>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let nav = menu(cx).await?;
    let paths = address::paths(cx).await?;
    let newest = products(
        cx,
        &ProductQuery::default()
            .sort(ProductSort::Newest)
            .per_page(NEW_ARRIVALS),
    )
    .await?;
    let picks = products(
        cx,
        &ProductQuery::default()
            .sort(ProductSort::TopRated)
            .exclude_all(newest.items.iter().map(|p| p.id.clone()))
            .per_page(PICKS),
    )
    .await?;
    let mut tiles = Vec::with_capacity(TILES);
    for node in nav.iter().take(TILES) {
        // The newest product in it that has a picture lends it.
        let recent = products(
            cx,
            &ProductQuery::default()
                .in_category(node.category.id.clone())
                .sort(ProductSort::Newest)
                .per_page(TILE_CANDIDATES),
        )
        .await?;
        tiles.push(Tile {
            node: node.clone(),
            image: recent
                .items
                .into_iter()
                .find_map(|p| p.images.into_iter().next()),
        });
    }
    let quotes = recent_reviews(cx, QUOTES).await?;
    let discover = nav
        .first()
        .map(|node| address::category_href(cx, paths, &node.category))
        .unwrap_or_else(|| href!(boutique::index).resolve(cx));

    // Who the shop is, for the name a search result shows.
    let home = lang::speaking(cx)
        .await
        .absolute(cx, &href!(index).resolve(cx));
    let structured = [
        organization_ld(SHOP_NAME, &home, Some(CONTACT_EMAIL)),
        website_ld(SHOP_NAME, &home),
    ];

    Ok(view! {
        document(
            title: SHOP_NAME,
            structured: &structured,
            nav: &nav,
            <section class="hero">
                <div class="wrap">
                    <span class="eyebrow">(t!(shop_tagline))</span>
                    <h1>(t!(home_hero_title))</h1>
                    <p>(t!(home_hero_text))</p>
                    <a class="btn" href=(discover) data-discover="">(t!(home_discover))</a>
                </div>
            </section>
            if !newest.items.is_empty() {
                <section class="block" data-section="nouveautes">
                    <div class="wrap">
                        <h2>(t!(home_new_arrivals))</h2>
                        <ul class="rail">
                            for product in &newest.items {
                                product_card(product: product)
                            }
                        </ul>
                    </div>
                </section>
            }
            <section class="block band">
                <div class="wrap prose">
                    <h2>(t!(home_selection_title))</h2>
                    <p>(t!(home_selection_text))</p>
                    <a class="btn" href=(href!(boutique::index))>(t!(home_selection_link))</a>
                </div>
            </section>
            if !tiles.is_empty() {
                <section class="block" data-section="categories">
                    <div class="wrap">
                        <h2>(t!(home_categories))</h2>
                        <ul class="tiles">
                            for tile in &tiles {
                                <li>
                                    <a class="tile" data-tile=(&tile.node.category.slug)
                                        href=(address::category_href(cx, paths, &tile.node.category))>
                                        if let Some(image) = &tile.image {
                                            product_image(image: image, attrs: attributes! {})
                                        }
                                        <span>(&tile.node.category.name) <small>(t!(home_tile_count, count: tile.node.product_count))</small></span>
                                    </a>
                                </li>
                            }
                        </ul>
                    </div>
                </section>
            }
            <section class="block band">
                <div class="wrap prose">
                    <h2>(t!(home_about_title))</h2>
                    <p>(t!(home_about_text))</p>
                    <a class="btn" href=(href!(qui_sommes_nous::index))>(t!(home_about_link))</a>
                </div>
            </section>
            if !quotes.is_empty() {
                <section class="block" data-section="avis">
                    <div class="wrap">
                        <h2>(t!(home_reviews_title))</h2>
                        <ul class="quotes">
                            for review in &quotes {
                                <li data-review=(&review.id)>
                                    <span class="stars" aria-label=(t!(rating_of_five, rating: review.rating))>(review.stars())</span>
                                    if let Some(title) = &review.title { <strong>" " (title)</strong> }
                                    <blockquote>(&review.body)</blockquote>
                                    <cite>(&review.author_name) " · " (t!(review_verified))</cite>
                                </li>
                            }
                        </ul>
                    </div>
                </section>
            }
            if !picks.items.is_empty() {
                <section class="block" data-section="selection">
                    <div class="wrap">
                        <h2>(t!(home_picks_title))</h2>
                        <ul class="rail">
                            for product in &picks.items {
                                product_card(product: product)
                            }
                        </ul>
                    </div>
                </section>
            }
            <section class="block">
                <div class="wrap">
                    reassurance()
                </div>
            </section>
        )
    })
}

/// The four promises every shop page repeats.
#[topcoat::view::component]
pub async fn reassurance(cx: &Cx) -> Result<impl View> {
    Ok(view! {
        <ul class="reassure" data-reassure="">
            <li>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M3 7h11v9H3zM14 10h4l3 3v3h-7z"/><circle cx="7" cy="18" r="1.6"/><circle cx="17" cy="18" r="1.6"/></svg>
                <strong>(t!(reassure_shipping_title))</strong>
                <p>(t!(reassure_shipping_text))</p>
            </li>
            <li>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><rect x="4" y="10" width="16" height="10" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/></svg>
                <strong>(t!(reassure_payment_title))</strong>
                <p>(t!(reassure_payment_text))</p>
            </li>
            <li>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M4 12a8 8 0 0 1 16 0v5a2 2 0 0 1-2 2h-2v-6h4M4 12v5a2 2 0 0 0 2 2h2v-6H4"/></svg>
                <strong>(t!(reassure_service_title))</strong>
                <p>(t!(reassure_service_text))</p>
            </li>
            <li>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M4 12a8 8 0 1 0 3-6.2"/><path d="M4 4v5h5"/></svg>
                <strong>(t!(reassure_returns_title))</strong>
                <p>(t!(reassure_returns_text))</p>
            </li>
        </ul>
    })
}

/// How a refusal (an [`OrderError`](timada_storefront::OrderError) code
/// carried in the URL) reads to a shopper.
pub fn say(code: &str) -> Worded<'static> {
    match code {
        "invalid_quantity" => t!(error_invalid_quantity),
        "too_many_lines" => t!(error_too_many_lines),
        "product_unavailable" => t!(error_product_unavailable),
        "out_of_stock" => t!(error_out_of_stock),
        "variant_required" => t!(error_variant_required),
        "variant_not_found" => t!(error_variant_not_found),
        "mixed_currencies" => t!(error_mixed_currencies),
        "empty_cart" => t!(error_empty_cart),
        "price_changed" => t!(error_price_changed),
        "payments_unavailable" => t!(error_payments_unavailable),
        "invoicing_not_configured" => t!(error_invoicing_not_configured),
        "oss_required_for_currency" => t!(error_oss_required_for_currency),
        "payment_failed" => t!(error_payment_failed),
        "payment_expired" => t!(error_payment_expired),
        "payment_returned_out_of_stock" => t!(error_payment_returned_out_of_stock),
        "payment_returned_discount_exhausted" => t!(error_payment_returned_discount_exhausted),
        "payment_returned_discount_already_used" => {
            t!(error_payment_returned_discount_already_used)
        }
        "invalid_email" => t!(error_invalid_email),
        "invalid_vat_number" => t!(error_invalid_vat_number),
        "shipping_unavailable" => t!(error_shipping_unavailable),
        "business_buyers_refused" => t!(error_business_buyers_refused),
        "invalid_ship_name" => t!(error_invalid_ship_name),
        "invalid_address" => t!(error_invalid_address),
        "invalid_country" => t!(error_invalid_country),
        "invalid_phone" => t!(error_invalid_phone),
        "missing_phone" => t!(error_missing_phone),
        "invalid_note" => t!(error_invalid_note),
        "cart_checked_out" => t!(error_cart_checked_out),
        "cart_not_found" => t!(error_cart_not_found),
        "too_many_carts" => t!(error_too_many_carts),
        "discount_unknown" => t!(error_discount_unknown),
        "discount_inactive" => t!(error_discount_inactive),
        "discount_currency" => t!(error_discount_currency),
        "discount_exhausted" => t!(error_discount_exhausted),
        "discount_already_used" => t!(error_discount_already_used),
        "offer_ended" => t!(error_offer_ended),
        "review_invalid_rating" => t!(error_review_invalid_rating),
        "review_invalid_title" => t!(error_review_invalid_title),
        "review_invalid_body" => t!(error_review_invalid_body),
        "review_too_many_photos" => t!(error_review_too_many_photos),
        "review_invalid_photo" => t!(error_review_invalid_photo),
        "review_storage_full" => t!(error_review_storage_full),
        "review_order_not_found" => t!(error_review_order_not_found),
        "review_not_purchased" => t!(error_review_not_purchased),
        "review_order_cancelled" => t!(error_review_order_cancelled),
        "review_order_not_paid" => t!(error_review_order_not_paid),
        "order_in_preparation" => t!(error_order_in_preparation),
        "not_cancellable" => t!(error_not_cancellable),
        "order_cancelled" => t!(error_order_cancelled),
        "cancellation_pending" => t!(error_cancellation_pending),
        "review_already_reviewed" => t!(error_review_already_reviewed),
        "review_not_pending" => t!(error_review_not_pending),
        "review_not_found" => t!(error_review_not_found),
        "review_photo_not_found" => t!(error_review_photo_not_found),
        "wrong_code" => t!(error_wrong_code),
        "code_expired" => t!(error_code_expired),
        "too_many_attempts" => t!(error_too_many_attempts),
        "code_recently_sent" => t!(error_code_recently_sent),
        "too_many_requests" => t!(error_too_many_requests),
        "too_many_addresses" => t!(error_too_many_addresses),
        "signed_out" => t!(error_signed_out),
        "store_unavailable" => t!(error_store_unavailable),
        "writes_paused" => t!(error_writes_paused),
        "write_not_taken" => t!(error_write_not_taken),
        _ => t!(error_unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::{write_money, write_rate};

    #[test]
    fn an_amount_is_written_as_its_reader_expects() {
        assert_eq!(write_money("fr", 1250, "EUR"), "12,50€");
        assert_eq!(
            write_money("fr", 123_456_705, "EUR"),
            "1\u{a0}234\u{a0}567,05€"
        );
        assert_eq!(write_money("fr", 5, "USD"), "0,05\u{a0}USD");
        assert_eq!(write_money("en", 1250, "EUR"), "€12.50");
        assert_eq!(write_money("en", 123_456_705, "EUR"), "€1,234,567.05");
        assert_eq!(write_money("en", 5, "USD"), "0.05\u{a0}USD");
        // A language with no way of its own yet reads as English does.
        assert_eq!(write_money("de", 1250, "EUR"), "€12.50");
    }

    #[test]
    fn a_rate_drops_the_decimals_it_does_not_need() {
        assert_eq!(write_rate("fr", 2000), "20");
        assert_eq!(write_rate("fr", 550), "5,5");
        assert_eq!(write_rate("en", 550), "5.5");
        assert_eq!(write_rate("en", 2125), "21.25");
    }
}
