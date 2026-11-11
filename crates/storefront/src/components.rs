//! The two components the library ships: everything else a page shows is the
//! merchant's own markup around [`Product`] values.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_view::{Attributes, View};
use topcoat_view_macro::{class, component, view};

use crate::context::money;
use crate::query::{Image, Product};

/// The price of a product in a `<span data-price>`, written as the store
/// writes amounts (`12.50 €`, or the host's own way:
/// [`StoreConfig::money`](crate::StoreConfig::money)), or nothing when it
/// has none. Every class and attribute comes from the caller:
/// `price(product: &p, attrs: attributes! { class="price" })`.
#[component]
pub async fn price(
    cx: &Cx,
    product: &Product,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let price = product.price_with(money(cx)).unwrap_or_default();
    Ok(view! {
        <span data-price="" class=(class) (attrs)>(price)</span>
    })
}

/// A product's picture as an `<img>`, with the address, the description and
/// the size the store knows. By default the listing rendition (400 px on
/// its longest side), which is what a card, a menu or a cart line shows;
/// `large: true` for the picture itself, as a product page shows it: the
/// page rendition, with a `srcset` that lets a phone take the smaller one
/// (`sizes` can be given in `attrs`; it defaults to the viewport up to
/// 1200 px). Loading is deferred, so a long gallery costs one request per
/// picture a shopper actually scrolls to. Every class comes from the
/// caller: `product_image(image: &photo, attrs: attributes! { class="photo" })`.
#[component]
pub async fn product_image(
    image: &Image,
    #[default] large: bool,
    #[default] mut attrs: Attributes,
) -> Result<impl View> {
    let class = class!(attrs.remove("class"));
    let (src, width, height) = if large {
        (&image.url, image.width, image.height)
    } else {
        (&image.thumb_url, image.thumb_width, image.thumb_height)
    };
    // A `srcset` only where the two renditions differ: a picture smaller
    // than the listing side is the same file twice, and width descriptors
    // on equal widths would have the browser misjudge its size.
    let two_sizes = large && image.thumb_width < image.width;
    let srcset = two_sizes.then(|| {
        format!(
            "{} {}w, {} {}w",
            image.thumb_url, image.thumb_width, image.url, image.width
        )
    });
    // The caller's `sizes`, in `attrs`, wins over the default.
    let sizes =
        (two_sizes && attrs.get("sizes").is_none()).then_some("(max-width: 1200px) 100vw, 1200px");
    Ok(view! {
        <img data-image=(&image.id) src=(src) srcset=(srcset) sizes=(sizes) alt=(&image.alt)
            width=(width) height=(height)
            loading="lazy" decoding="async" class=(class) (attrs)>
    })
}
