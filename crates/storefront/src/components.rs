//! The components the library ships. There is one: everything else a page
//! shows is the merchant's own markup around [`Product`] values.

use topcoat_core::error::Result;
use topcoat_view::{Attributes, View};
use topcoat_view_macro::{class, component, view};

use crate::query::Product;

/// The price of a product in a `<span data-price>`, formatted (`12.50 EUR`),
/// or nothing when it has none. Every class and attribute comes from the
/// caller: `price(product: &p, attrs: attributes! { class="price" })`.
#[component]
pub async fn price(product: &Product, #[default] mut attrs: Attributes) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <span data-price="" class=(class) (attrs)>(product.price().unwrap_or_default())</span>
    })
}
