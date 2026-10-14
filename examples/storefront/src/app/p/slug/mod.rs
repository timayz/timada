use timada_storefront::{categories, price, product_categories, related_products, require_product};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param},
    view::{View, attributes, view},
};

use crate::app::{catalog_grid, document};

module_param!(pub slug);

/// How many products the "you may also like" row shows.
const RELATED: u32 = 4;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let product = require_product(cx, path_param::<Slug>(cx)).await?;
    let nav = categories(cx).await?;
    let related = related_products(cx, &product, RELATED).await?;
    // The first category it is filed under carries the breadcrumb.
    let crumb = product_categories(cx, &product).await?.into_iter().next();

    Ok(view! {
        document(
            title: &product.name,
            nav: &nav,
            <nav class="crumbs" aria-label="Breadcrumb" data-crumbs="">
                <a href=(href!(crate::app::index))>"Catalog"</a>
                if let Some(crumb) = &crumb {
                    " / "
                    <a href=(href!(crate::app::c::slug::index, crate::app::c::slug::Slug(crumb.slug.clone())))>(&crumb.name)</a>
                }
                " / " (&product.name)
            </nav>
            <h1 data-product=(&product.slug)>(&product.name)</h1>
            <p>price(product: &product, attrs: attributes! { class="price" })</p>
            <p class="description">(&product.description)</p>
            if !related.is_empty() {
                <section class="related">
                    <h2>"You may also like"</h2>
                    catalog_grid(products: &related)
                </section>
            }
        )
    })
}
