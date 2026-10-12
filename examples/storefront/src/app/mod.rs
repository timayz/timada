//! The pages: the catalog at `/`, a product at `/p/{slug}`, and the
//! library's `/store/status`.

pub mod p;

use timada_storefront::{ProductQuery, RouterBuilderStoreExt, Store, price, products};
use topcoat::{
    Result,
    context::Cx,
    router::{Router, RouterBuilderDiscoverExt, href, module_router, page, query_params},
    view::{Child, View, attributes, view},
};

pub fn router(store: Store) -> Router {
    module_router!().discover().store(store).build()
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
pub async fn document(title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title)</title>
                <style>
                    "body { font: 16px/1.5 system-ui, sans-serif; margin: 0 auto; max-width: 56rem; padding: 2rem 1rem; color: #222; }"
                    "header { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 2rem; }"
                    "ul.catalog { list-style: none; padding: 0; display: grid; gap: 1rem; grid-template-columns: repeat(auto-fill, minmax(14rem, 1fr)); }"
                    "ul.catalog li { border: 1px solid #ddd; border-radius: .5rem; padding: 1rem; }"
                    ".price { font-weight: 600; }"
                    "nav.pages { display: flex; gap: 1rem; margin-top: 1.5rem; }"
                    "p.description { white-space: pre-line; }"
                </style>
            </head>
            <body>
                <header>
                    <a href=(href!(index))><strong>"The Mug Shop"</strong></a>
                    <form method="get" action=(href!(index))>
                        <input name="q" type="search" placeholder="Search" aria-label="Search">
                    </form>
                </header>
                <main>(child)</main>
            </body>
        </html>
    })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CatalogQuery>(cx)?;
    let mut wanted = ProductQuery::default().page(query.page.unwrap_or(1));
    if let Some(q) = &query.q {
        wanted = wanted.search(q.as_str());
    }
    let page = products(cx, &wanted).await?;
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
            title: "The Mug Shop",
            <h1>"Catalog"</h1>
            if page.items.is_empty() {
                <p>"Nothing on sale right now."</p>
            }
            <ul class="catalog">
                for product in &page.items {
                    <li data-product=(&product.slug)>
                        <a href=(href!(p::slug::index, p::slug::Slug(product.slug.clone())))>(&product.name)</a>
                        <div>price(product: product, attrs: attributes! { class="price" })</div>
                    </li>
                }
            </ul>
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
