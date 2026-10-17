//! The two leaf components: a price and a picture. The forms and the other
//! behaviour-carrying elements are in [`forms`](crate::forms); everything
//! else a page shows is the merchant's own markup around [`Product`] values.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_view::{Attributes, View};
use topcoat_view_macro::{class, component, view};

use crate::query::{Image, Product};

/// The price of a product in a `<span data-price>`, written the shop's way
/// (`12.50 EUR`, or a range across the variants), or nothing when it has
/// none. Every class and attribute comes from the caller:
/// `price(product: &p, attrs: attributes! { class="price" })`.
#[component]
pub async fn price(
    cx: &Cx,
    product: &Product,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let price = crate::money_format(cx).price_line(product);
    Ok(view! {
        <span data-price="" class=(class) (attrs)>(price)</span>
    })
}

/// A product's picture as an `<img>`, with the address, the description and
/// the size the store knows. Loading is deferred, so a long gallery costs
/// one request per picture a shopper actually scrolls to. Every class comes
/// from the caller:
/// `product_image(image: &photo, attrs: attributes! { class="photo" })`.
#[component]
pub async fn product_image(image: &Image, #[default] mut attrs: Attributes) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    Ok(view! {
        <img data-image=(&image.id) src=(&image.url) alt=(&image.alt)
            width=(image.width) height=(image.height)
            loading="lazy" decoding="async" class=(class) (attrs)>
    })
}
