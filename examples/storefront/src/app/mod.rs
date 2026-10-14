//! The pages: the catalog at `/`, a category at `/c/{slug}`, a product at
//! `/p/{slug}`, and the library's `/store/status`.

pub mod c;
pub mod p;

use timada_storefront::{
    CategoryNode, ProductQuery, ProductSort, RouterBuilderStoreExt, Store, categories, price,
    products,
};
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
                </style>
            </head>
            <body>
                <header>
                    <a href=(href!(index))><strong>"The Cat Shop"</strong></a>
                    <form method="get" action=(href!(index))>
                        <input name="q" type="search" placeholder="Search" aria-label="Search">
                    </form>
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
                    <a href=(href!(p::slug::index, p::slug::Slug(product.slug.clone())))>(&product.name)</a>
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
