use timada_storefront::{price, require_product};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param},
    view::{View, attributes, view},
};

use crate::app::document;

module_param!(pub slug);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let product = require_product(cx, path_param::<Slug>(cx)).await?;
    Ok(view! {
        document(
            title: &product.name,
            <p><a href=(href!(crate::app::index))>"← Catalog"</a></p>
            <h1 data-product=(&product.slug)>(&product.name)</h1>
            <p>price(product: &product, attrs: attributes! { class="price" })</p>
            <p class="description">(&product.description)</p>
        )
    })
}
