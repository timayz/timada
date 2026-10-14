//! The pages: the catalog at `/`, a category at `/c/{slug}`, a product at
//! `/p/{slug}`, the cart at `/cart`, the checkout at `/checkout`, a placed
//! order at `/orders/{order_id}`, and the library's `/store/status`.

pub mod c;
pub mod cart;
pub mod checkout;
pub mod orders;
pub mod p;

use timada_storefront::{
    CategoryNode, ProductQuery, ProductSort, RouterBuilderStoreExt, Store, categories, price,
    product_image, products,
};
use topcoat::{
    Result,
    context::Cx,
    cookie::{Key, RouterBuilderCookieExt},
    router::{Router, RouterBuilderDiscoverExt, href, module_router, page, query_params},
    view::{Child, View, attributes, view},
};

pub fn router(store: Store, cookie_key: Key) -> Router {
    module_router!()
        .discover()
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
/// library ships no markup and no classes, the merchant decides.
#[topcoat::view::component]
pub async fn document(
    title: &str,
    #[default] nav: &[CategoryNode],
    #[default] child: Child<'_>,
) -> Result<impl View> {
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
                    "nav.pages { display: flex; gap: 1rem; margin-top: 1.5rem; }"
                    "p.description { white-space: pre-line; }"
                    "nav.sections { border-bottom: 1px solid #ddd; margin-bottom: 2rem; padding-bottom: 1rem; }"
                    "nav.sections > ul { list-style: none; padding: 0; margin: 0; display: flex; flex-wrap: wrap; gap: 1.5rem; }"
                    "nav.sections ul ul { display: block; margin: .25rem 0 0; }"
                    "nav.sections summary { cursor: pointer; }"
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
                </style>
            </head>
            <body>
                <header>
                    <a href=(href!(index))><strong>"The Cat Shop"</strong></a>
                    <form method="get" action=(href!(index))>
                        <input name="q" type="search" placeholder="Search" aria-label="Search">
                    </form>
                    <a href=(href!(cart::index)) data-cart-link="">"Cart"</a>
                </header>
                if !nav.is_empty() {
                    <nav class="sections" aria-label="Categories">
                        <ul>
                            for node in nav {
                                <li>
                                    if node.children.is_empty() {
                                        menu_link(node: node)
                                    } else {
                                        <details>
                                            <summary>menu_link(node: node)</summary>
                                            <ul>
                                                for child in &node.children {
                                                    <li>menu_link(node: child)</li>
                                                }
                                            </ul>
                                        </details>
                                    }
                                </li>
                            }
                        </ul>
                    </nav>
                }
                <main>(child)</main>
            </body>
        </html>
    })
}

/// One entry of the menu: the category and how many products are in it.
#[topcoat::view::component]
pub async fn menu_link(node: &CategoryNode) -> Result<impl View> {
    Ok(view! {
        <a data-category=(&node.category.slug)
            href=(href!(c::slug::index, c::slug::Slug(node.category.slug.clone())))>
            (&node.category.name) " (" (node.product_count) ")"
        </a>
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
    let nav = categories(cx).await?;
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
        "invalid_email" => "Enter a valid e-mail address.",
        "invalid_ship_name" => "Enter the name to ship to.",
        "invalid_address" => "Enter the street, postal code and city.",
        "invalid_country" => "Enter the country as a two-letter code, such as FR.",
        "invalid_phone" => "The phone number is too long.",
        "invalid_note" => "The note is too long.",
        "cart_not_found" | "cart_checked_out" => "This cart was already ordered.",
        _ => "Something went wrong. Please try again.",
    }
}
