use timada_storefront::{
    categories, price, product_categories, product_image, related_products, require_product,
};
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
            if !product.images.is_empty() {
                <ul class="gallery" data-gallery="">
                    for image in &product.images {
                        <li>product_image(image: image, attrs: attributes! { class="photo" })</li>
                    }
                </ul>
            }
            <p>price(product: &product, attrs: attributes! { class="price" })</p>

            if !product.options.is_empty() {
                // No cart yet, so the menus say what there is and the table
                // below prices every combination. Both are plain HTML: a
                // shopper with no JavaScript sees the same thing.
                <form class="picker">
                    for option in &product.options {
                        <p>
                            <label for=(format!("option-{}", option.id))>(&option.name)</label>
                            <select id=(format!("option-{}", option.id)) name=(&option.id) data-option=(&option.name)>
                                for value in &option.values {
                                    <option value=(value)>(value)</option>
                                }
                            </select>
                        </p>
                    }
                </form>
                <table class="variants" data-variants="">
                    <thead>
                        <tr>
                            for option in &product.options {
                                <th scope="col">(&option.name)</th>
                            }
                            <th scope="col">"Price"</th>
                        </tr>
                    </thead>
                    <tbody>
                        for variant in &product.variants {
                            <tr data-variant=(&variant.id) data-price=(variant.amount_cents)>
                                for option in &product.options {
                                    <td>(variant.value_of(&option.id).unwrap_or("—"))</td>
                                }
                                <td class="price">(variant.price())</td>
                            </tr>
                        }
                    </tbody>
                </table>
            }

            <p class="description">(&product.description)</p>

            if !product.attributes.is_empty() {
                <h2>"Additional information"</h2>
                <table class="attributes" data-attributes="">
                    <tbody>
                        for attribute in &product.attributes {
                            <tr data-attribute=(&attribute.name)>
                                <th scope="row">(&attribute.name)</th>
                                <td>(&attribute.value)</td>
                            </tr>
                        }
                    </tbody>
                </table>
            }
            if !related.is_empty() {
                <section class="related">
                    <h2>"You may also like"</h2>
                    catalog_grid(products: &related)
                </section>
            }
        )
    })
}
