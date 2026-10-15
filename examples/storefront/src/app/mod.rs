//! The pages: the catalog at `/`, a category at `/c/{slug}`, a product at
//! `/p/{slug}`, the cart at `/cart`, the checkout at `/checkout`, a placed
//! order at `/orders/{order_id}`, the shopper's account at `/account`, and
//! the library's `/store/status`.

pub mod account;
pub mod c;
pub mod cart;
pub mod checkout;
pub mod menu;
pub mod orders;
pub mod p;
pub mod reviews;

use timada_storefront::{
    CategoryNode, ProductQuery, ProductSort, RouterBuilderStoreExt, Store, menu, price,
    product_image, products,
};
use topcoat::{
    Result,
    context::Cx,
    cookie::{Key, RouterBuilderCookieExt},
    router::{
        BodyLimit, Router, RouterBuilderDiscoverExt, href, module_router, page, query_params,
    },
    view::{Child, Unescaped, View, attributes, view},
};

use crate::shopper::accounts_enabled;
use cart::drawer::{CART_SCRIPT, DRAWER_CSS, cart_drawer};
use menu::{MENU_CSS, MENU_SCRIPT, site_menu};

/// A review carries up to three photos of up to 8 MiB each.
const REVIEW_LIMIT: usize = 26 * 1024 * 1024;

pub fn router(store: Store, cookie_key: Key) -> Router {
    module_router!()
        .discover()
        .layer(BodyLimit::max(REVIEW_LIMIT).at("/orders"))
        .cookies()
        .app_context(cookie_key)
        .store(store)
        .build()
}

#[topcoat::router::query_params(error = bad_request)]
struct CatalogQuery {
    q: Option<String>,
    page: Option<u32>,
}

#[derive(serde::Serialize)]
struct CatalogLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    q: Option<&'a str>,
    page: u32,
}

/// The HTML shell every page shares. The look is a few inline rules: the
/// library ships no markup and no classes, the merchant decides. Every page
/// but the cart's and the checkout's (`no_drawer`, they show the cart
/// already) carries the cart drawer.
#[topcoat::view::component]
pub async fn document(
    cx: &Cx,
    title: &str,
    #[default] nav: &[CategoryNode],
    #[default] no_drawer: bool,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let cart = cart::current_cart(cx).await?;
    let count = cart
        .as_ref()
        .map(|cart| cart.item_count)
        .unwrap_or_default();
    let accounts = accounts_enabled(cx);
    let signed_in = accounts && crate::shopper::signed_in(cx).await.is_some();
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title)</title>
                <style>
                    "body { font: 16px/1.5 system-ui, sans-serif; margin: 0 auto; max-width: 56rem; padding: 2rem 1rem; color: #222; }"
                    "header { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 1rem; }"
                    "ul.catalog { list-style: none; padding: 0; display: grid; gap: 1rem; grid-template-columns: repeat(auto-fill, minmax(14rem, 1fr)); }"
                    "ul.catalog li { border: 1px solid #ddd; border-radius: .5rem; padding: 1rem; }"
                    "ul.catalog a { display: block; text-decoration: none; color: inherit; }"
                    "img.thumb { display: block; width: 100%; height: 10rem; object-fit: cover; border-radius: .25rem; margin-bottom: .5rem; background: #f4f4f4; }"
                    "ul.gallery { list-style: none; padding: 0; display: grid; gap: .5rem; grid-template-columns: repeat(auto-fill, minmax(8rem, 1fr)); margin-bottom: 1.5rem; }"
                    "ul.gallery img { width: 100%; height: 8rem; object-fit: cover; border-radius: .25rem; background: #f4f4f4; }"
                    "ul.gallery li:first-child { grid-column: 1 / -1; }"
                    "ul.gallery li:first-child img { height: 20rem; }"
                    ".price { font-weight: 600; }"
                    ".rating { color: #b45309; }"
                    ".reviews li { margin-bottom: 1rem; }"
                    ".reviews img { width: 6rem; height: 6rem; object-fit: cover; margin-right: .5rem; }"
                    "nav.pages { display: flex; gap: 1rem; margin-top: 1.5rem; }"
                    "p.description { white-space: pre-line; }"
                    "nav.crumbs { color: #666; font-size: .875rem; margin-bottom: 1rem; }"
                    "nav.sort { margin-bottom: 1rem; }"
                    "section.related { margin-top: 2.5rem; }"
                    "form.picker { margin: 1rem 0; }"
                    "form.picker label { display: inline-block; min-width: 6rem; }"
                    "table.variants, table.attributes { border-collapse: collapse; margin: 1rem 0; min-width: 20rem; }"
                    "table.variants th, table.variants td, table.attributes th, table.attributes td { border: 1px solid #ddd; padding: .4rem .75rem; text-align: left; }"
                    "table.attributes th { background: #fafafa; }"
                    "table.cart { border-collapse: collapse; width: 100%; margin: 1rem 0; }"
                    "table.cart th, table.cart td { border-bottom: 1px solid #ddd; padding: .5rem; text-align: left; vertical-align: middle; }"
                    "table.cart img { width: 4rem; height: 4rem; object-fit: cover; border-radius: .25rem; }"
                    "table.cart input[type=number] { width: 4rem; }"
                    "p.notice { padding: .75rem; border-radius: .25rem; background: #fff4e5; border: 1px solid #f5c27a; }"
                    "form.checkout label { display: block; margin-top: .75rem; }"
                    "form.checkout input, form.checkout textarea { width: 100%; max-width: 28rem; padding: .4rem; }"
                    "button.primary { margin-top: 1rem; padding: .6rem 1.2rem; font-size: 1rem; }"
                    ".vh { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }"
                    // The checkout: steps, then the form beside the cart.
                    "ol.steps { list-style: none; display: flex; justify-content: center; gap: 0; padding: 0; margin: 0 auto 1.5rem; max-width: 40rem; counter-reset: step; }"
                    "ol.steps li { flex: 1; text-align: center; color: #888; position: relative; padding-top: 1.75rem; font-size: .9rem; }"
                    "ol.steps li::before { counter-increment: step; content: counter(step); position: absolute; top: 0; left: 50%; transform: translateX(-50%); width: 1.4rem; height: 1.4rem; line-height: 1.4rem; border-radius: 50%; background: #f6c9cc; color: #fff; font-size: .75rem; font-weight: 600; z-index: 1; }"
                    "ol.steps li::after { content: ''; position: absolute; top: .65rem; left: 0; right: 0; height: 2px; background: #f6c9cc; }"
                    "ol.steps li.done::before { content: '✓'; background: #e8606a; }"
                    "ol.steps li.done::after, ol.steps li[aria-current]::after { background: #e8606a; }"
                    "ol.steps li[aria-current] { color: #3d3b5c; font-weight: 600; }"
                    "ol.steps li[aria-current]::before { background: #e8606a; }"
                    "ol.steps a { color: inherit; font-weight: 600; }"
                    ".checkout-grid { display: grid; gap: 1.5rem; }"
                    "@media (min-width: 900px) { body:has(.checkout-grid) { max-width: 68rem; } .checkout-grid { grid-template-columns: minmax(0, 1fr) 22rem; align-items: start; } .checkout-main { grid-column: 1; grid-row: 1; } .checkout-aside { grid-column: 2; grid-row: 1; position: sticky; top: 1rem; } }"
                    ".checkout-grid h2 { font-size: 1.2rem; text-transform: uppercase; color: #3d3b5c; margin: 0 0 1rem; letter-spacing: .02em; }"
                    ".card { background: #fff; border: 1px solid #eee; border-radius: .5rem; padding: 1.25rem 1.5rem; margin-bottom: 1.25rem; box-shadow: 0 1px 3px rgba(0,0,0,.04); }"
                    "form.checkout { margin: 0; }"
                    "form.checkout label { display: block; margin-top: .75rem; font-size: .9rem; color: #555; }"
                    "form.checkout input, form.checkout textarea, form.checkout select { width: 100%; max-width: none; padding: .65rem .75rem; border: 1px solid #ccc; border-radius: .3rem; font: inherit; box-sizing: border-box; background: #fff; }"
                    "form.checkout .row { display: grid; gap: .75rem; grid-template-columns: repeat(auto-fit, minmax(9rem, 1fr)); }"
                    "form.checkout details.more summary { color: #e8606a; cursor: pointer; margin-top: .6rem; font-size: .9rem; }"
                    ".hint { color: #666; font-size: .875rem; }"
                    "button.place { display: block; width: 100%; margin-top: 1.5rem; padding: 1rem; border: 0; border-radius: 2rem; background: #f28b8f; color: #fff; font-size: 1.1rem; font-weight: 600; letter-spacing: .03em; text-transform: uppercase; cursor: pointer; box-shadow: 0 6px 16px rgba(242,139,143,.35); }"
                    "button.place:disabled { opacity: .6; cursor: progress; }"
                    "#payment-element { margin: 1rem 0; }"
                    ".express { border: 1px solid #ddd; border-radius: .5rem; padding: 1rem 1.25rem 0; margin-bottom: 1.25rem; }"
                    ".express h2 { text-align: center; font-size: .9rem !important; text-transform: none !important; font-weight: 400; color: #555 !important; }"
                    "p.or { display: flex; align-items: center; gap: 1rem; color: #888; font-size: .85rem; margin: 1rem 0; }"
                    "p.or::before, p.or::after { content: ''; flex: 1; border-top: 1px solid #ddd; }"
                    ".summary ul.lines { list-style: none; padding: 0; margin: 0; }"
                    ".summary ul.lines li { display: grid; grid-template-columns: 3.5rem 1fr auto; gap: .75rem; padding: .6rem 0; font-size: .9rem; color: #3d3b5c; }"
                    ".summary .pic { position: relative; width: 3.5rem; height: 3.5rem; border: 1px solid #eee; border-radius: .25rem; background: #f4f4f4; }"
                    ".summary .pic img { width: 100%; height: 100%; object-fit: cover; border-radius: .25rem; }"
                    ".summary .badge { position: absolute; top: -.5rem; left: -.5rem; min-width: 1.2rem; height: 1.2rem; line-height: 1.2rem; text-align: center; border-radius: 1rem; background: #777; color: #fff; font-size: .7rem; }"
                    ".summary .qty { display: flex; align-items: center; margin-top: .4rem; }"
                    ".summary .qty form { margin: 0; }"
                    ".summary .qty button, .summary .qty span { width: 1.6rem; height: 1.6rem; border: 1px solid #ddd; background: #fff; font-size: .8rem; text-align: center; line-height: 1.5rem; padding: 0; }"
                    ".summary .amount { text-align: right; }"
                    ".summary .amount form { margin-top: .4rem; }"
                    ".summary button.remove { border: 1px solid #bbb; border-radius: 50%; width: 1.2rem; height: 1.2rem; line-height: 1rem; padding: 0; background: #fff; color: #888; cursor: pointer; }"
                    ".summary details.promo { margin: 1rem 0; border-bottom: 1px solid #eee; padding-bottom: 1rem; }"
                    ".summary details.promo summary { color: #e8606a; font-weight: 600; cursor: pointer; font-size: .9rem; }"
                    ".summary form.discount { display: flex; gap: .5rem; margin-top: .75rem; }"
                    ".summary form.discount label { flex: 1; }"
                    ".summary form.discount input { width: 100%; padding: .5rem; box-sizing: border-box; }"
                    "dl.totals { display: grid; grid-template-columns: 1fr auto; gap: .4rem 0; margin: 0; font-size: .9rem; color: #3d3b5c; }"
                    "dl.totals dd { margin: 0; text-align: right; }"
                    "dl.totals .total { border-top: 1px solid #eee; padding-top: .75rem; margin-top: .4rem; font-size: 1.2rem; }"
                    "dl.totals dd.total small { display: block; font-size: .75rem; font-weight: 600; }"
                    "section.reassure h2, section.reviews-aside h2 { text-transform: none !important; font-weight: 500; }"
                    "section.reassure ul { list-style: none; padding: 0; }"
                    "section.reassure li { padding-left: 1.5rem; position: relative; margin: .4rem 0; color: #3d3b5c; }"
                    "section.reassure li::before { content: '✓'; position: absolute; left: 0; color: #e8606a; font-weight: 700; }"
                    "figure.review { background: #fff; border: 1px solid #eee; margin: 0 0 1rem; padding: .75rem 1rem; font-size: .85rem; }"
                    "figure.review figcaption { display: flex; align-items: center; gap: .5rem; font-weight: 600; }"
                    "figure.review .avatar { width: 2rem; height: 2rem; border-radius: 50%; background: #fde7cf; display: inline-grid; place-items: center; font-size: .75rem; }"
                    "figure.review .rating { color: #00b67a; margin: .4rem 0; }"
                    "figure.review blockquote { margin: .25rem 0 0; }"
                    "p.returning { margin: 0 0 1.25rem; padding: .75rem 1rem; border: 1px solid #eee; border-radius: .5rem; background: #fafafa; }"
                    "p.returning a { color: #e8606a; font-weight: 600; }"
                    "ul.saved { list-style: none; padding: 0; display: grid; gap: .5rem; }"
                    "ul.saved li { border: 1px solid #ddd; border-radius: .4rem; padding: .6rem .8rem; }"
                    "ul.saved li[aria-current] { border-color: #e8606a; }"
                    "ul.saved form { display: inline; }"
                    "form.checkout label.check { display: flex; gap: .5rem; align-items: center; }"
                    "form.checkout label.check input { width: auto; }"
                    "form.sign-in label { display: block; margin-top: .75rem; }"
                    "form.sign-in input { padding: .5rem; width: 100%; max-width: 20rem; }"
                    "table.history { border-collapse: collapse; width: 100%; }"
                    "table.history th, table.history td { border-bottom: 1px solid #ddd; padding: .5rem; text-align: left; }"
                    "[hidden] { display: none !important; }"
                </style>
                <style>(Unescaped::new_unchecked(DRAWER_CSS))</style>
                <style>(Unescaped::new_unchecked(MENU_CSS))</style>
            </head>
            <body>
                <header>
                    if !nav.is_empty() {
                        <a class="menu-toggle" href="#menu" aria-controls="menu">"Menu"</a>
                    }
                    <a href=(href!(index))><strong>"The Cat Shop"</strong></a>
                    <form method="get" action=(href!(index))>
                        <input name="q" type="search" placeholder="Search" aria-label="Search">
                    </form>
                    if accounts {
                        <a class="account-link" href=(href!(account::index)) data-account-link="">
                            if signed_in { "Account" } else { "Sign in" }
                        </a>
                    }
                    <a class="cart-link" href=(href!(cart::index)) data-cart-link="" aria-controls="cart">
                        "Cart"
                        <span class="count" data-cart-count="" hidden=(count == 0)>(count)</span>
                    </a>
                </header>
                if !nav.is_empty() {
                    site_menu(nav: nav)
                    <script>(Unescaped::new_unchecked(MENU_SCRIPT))</script>
                }
                <main>(child)</main>
                if !no_drawer {
                    cart_drawer(cart: cart.as_ref())
                    <script>(Unescaped::new_unchecked(CART_SCRIPT))</script>
                }
            </body>
        </html>
    })
}

/// A grid of product cards: the catalog and a category page share it.
#[topcoat::view::component]
pub async fn catalog_grid(products: &[timada_storefront::Product]) -> Result<impl View> {
    Ok(view! {
        <ul class="catalog">
            for product in products {
                <li data-product=(&product.slug)>
                    <a href=(href!(p::slug::index, p::slug::Slug(product.slug.clone())))>
                        if let Some(image) = product.image() {
                            product_image(image: image, attrs: attributes! { class="thumb" })
                        }
                        <span>(&product.name)</span>
                    </a>
                    <div>price(product: product, attrs: attributes! { class="price" })</div>
                    if let (Some(rating), Some(whole)) = (product.rating(), product.rating_stars()) {
                        <div class="rating" data-rating=(format!("{rating:.1}")) aria-label=(format!("{rating:.1} out of 5, {} reviews", product.review_count))>
                            (timada_storefront::stars(whole)) " (" (product.review_count) ")"
                        </div>
                    }
                </li>
            }
        </ul>
    })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CatalogQuery>(cx)?;
    let mut wanted = ProductQuery::default()
        .page(query.page.unwrap_or(1))
        .sort(ProductSort::Newest);
    if let Some(q) = &query.q {
        wanted = wanted.search(q.as_str());
    }
    let page = products(cx, &wanted).await?;
    let nav = menu(cx).await?;
    let link = move |number: u32| {
        href!(index)
            .query(CatalogLink {
                q: wanted.q.as_deref(),
                page: number,
            })
            .resolve(cx)
    };

    Ok(view! {
        document(
            title: "The Cat Shop",
            nav: &nav,
            <h1>"Catalog"</h1>
            if page.items.is_empty() {
                <p>"Nothing on sale right now."</p>
            }
            catalog_grid(products: &page.items)
            if page.pages() > 1 {
                <nav class="pages" aria-label="Pages">
                    if page.has_prev() { <a href=(link(page.page - 1))>"Previous"</a> }
                    <span>"Page " (page.page) " of " (page.pages())</span>
                    if page.has_next() { <a href=(link(page.page + 1))>"Next"</a> }
                </nav>
            }
        )
    })
}

/// How a refusal (an [`OrdersError`](timada_storefront::OrdersError) code
/// carried in the URL) reads to a shopper.
pub fn say(code: &str) -> &'static str {
    match code {
        "invalid_quantity" => "Choose a quantity from 1 to 99.",
        "too_many_lines" => "Your cart is full: order what is in it first.",
        "product_unavailable" => "Sorry, this product is no longer on sale.",
        "variant_required" => "Choose an option first.",
        "variant_not_found" => "This combination is not sold.",
        "mixed_currencies" => "This product is sold in another currency than your cart.",
        "empty_cart" => "Your cart is empty.",
        "price_changed" => {
            "A price changed since you saw it: check the new total and confirm again."
        }
        "payments_unavailable" => "Payments are not available right now. Please try again later.",
        "invoicing_not_configured" => {
            "This shop cannot take orders yet: it has not finished setting up. Please try again later."
        }
        "payment_failed" => "The payment did not go through. Try again or use another card.",
        "payment_expired" => "This payment is no longer open. Please check out again.",
        "invalid_email" => "Enter a valid e-mail address.",
        "invalid_vat_number" => {
            "Enter an EU VAT number: its country prefix, then letters and digits."
        }
        "business_buyers_refused" => {
            "This shop does not sell to businesses: leave the VAT number blank."
        }
        "invalid_ship_name" => "Enter the name to ship to.",
        "invalid_address" => "Enter the street, postal code and city.",
        "invalid_country" => "Enter the country as a two-letter code, such as FR.",
        "invalid_phone" => "The phone number is too long.",
        "invalid_note" => "The note is too long.",
        "cart_not_found" | "cart_checked_out" => "This cart was already ordered.",
        "discount_unknown" => "This discount code is not valid.",
        "discount_inactive" => "This discount code cannot be used right now.",
        "discount_currency" => "This discount code is for another currency.",
        "discount_exhausted" => "This discount code has been used as many times as it can be.",
        "discount_already_used" => "You have already used this discount code.",
        "review_invalid_rating" => "Choose from one to five stars.",
        "review_invalid_title" => "The title is at most 100 characters.",
        "review_invalid_body" => "Say a little more: 10 to 2000 characters.",
        "review_too_many_photos" => "Attach at most three photos.",
        "review_invalid_photo" => "Attach JPEG, PNG, WebP or GIF photos of at most 8 MiB.",
        "review_order_not_found" => "This order cannot be reviewed.",
        "review_not_purchased" => "This product is not part of this order.",
        "review_order_cancelled" => "A cancelled order cannot be reviewed.",
        "review_already_reviewed" => "You have already reviewed this product.",
        "wrong_code" => "This is not the code we sent. Check the e-mail and try again.",
        "code_expired" => "This code has expired: ask for a new one.",
        "too_many_attempts" => "Too many wrong codes: ask for a new one.",
        "code_recently_sent" => "A code was sent less than a minute ago: check your inbox.",
        "too_many_addresses" => "Your account keeps ten addresses: remove one first.",
        "signed_out" => "You were signed out. Please sign in again.",
        _ => "Something went wrong. Please try again.",
    }
}
